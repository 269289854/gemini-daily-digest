//! Shared HTTP/2 singleton `reqwest::Client`, global bounded concurrency
//! semaphore, `MAX_FEED_BYTES` streaming reader, and conditional header helpers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use tokio::sync::Semaphore;

use crate::config::CollectorTuning;
use crate::constants::{
    HEADER_ETAG, HEADER_IF_MODIFIED_SINCE, HEADER_IF_NONE_MATCH, HEADER_LAST_MODIFIED,
};
use crate::error::CoreError;
use crate::traits::ConditionalFetchHeaders;

const INITIAL_BUFFER_CAP_BYTES: usize = 64 * 1024;

static DEFAULT_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static PROXIED_HTTP_CLIENTS: OnceLock<Mutex<HashMap<String, reqwest::Client>>> = OnceLock::new();
static GLOBAL_FETCH_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn build_http_client(
    proxy_url: Option<&str>,
    tuning: &CollectorTuning,
) -> Result<reqwest::Client, CoreError> {
    let mut builder = reqwest::Client::builder()
        .use_rustls_tls()
        .gzip(true)
        .brotli(true)
        .user_agent(&tuning.user_agent)
        .pool_idle_timeout(tuning.pool_idle_timeout)
        .timeout(tuning.request_timeout)
        .redirect(reqwest::redirect::Policy::none());

    if let Some(raw_proxy) = proxy_url {
        let proxy = reqwest::Proxy::all(raw_proxy).map_err(|err| {
            CoreError::InvalidConfig(format!("invalid outbound proxy URL: {err}"))
        })?;
        builder = builder.proxy(proxy);
    }

    builder
        .build()
        .map_err(|err| CoreError::Internal(format!("failed to build HTTP client: {err}")))
}

/// Returns the shared HTTP/2-capable `reqwest::Client` singleton, or a cached
/// proxied client when `outbound_proxy_url` is provided.
pub fn shared_http_client(
    outbound_proxy_url: Option<&str>,
    tuning: &CollectorTuning,
) -> Result<reqwest::Client, CoreError> {
    let normalized_proxy = outbound_proxy_url
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match normalized_proxy {
        None => {
            if let Some(existing) = DEFAULT_HTTP_CLIENT.get() {
                return Ok(existing.clone());
            }
            let built = build_http_client(None, tuning)?;
            let _ = DEFAULT_HTTP_CLIENT.set(built.clone());
            Ok(DEFAULT_HTTP_CLIENT.get().cloned().unwrap_or(built))
        }
        Some(proxy_url) => {
            let cache = PROXIED_HTTP_CLIENTS.get_or_init(|| Mutex::new(HashMap::new()));
            {
                let guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
                if let Some(existing) = guard.get(proxy_url) {
                    return Ok(existing.clone());
                }
            }
            let built = build_http_client(Some(proxy_url), tuning)?;
            let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
            let entry = guard
                .entry(proxy_url.to_owned())
                .or_insert_with(|| built.clone());
            Ok(entry.clone())
        }
    }
}

/// Returns the shared global semaphore bounding concurrent outbound fetches.
#[must_use]
pub fn global_fetch_semaphore(concurrency: usize) -> Arc<Semaphore> {
    let permits = concurrency.max(1);
    GLOBAL_FETCH_SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(permits)))
        .clone()
}

/// Streams a `reqwest::Response` body up to `max_bytes`, aborting immediately
/// if `Content-Length` or cumulative streamed bytes exceed the cap.
pub async fn read_response_bytes_capped(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Bytes, CoreError> {
    if let Some(content_length) = response.content_length() {
        if content_length > max_bytes as u64 {
            return Err(CoreError::Upstream(format!(
                "response content-length ({content_length} bytes) exceeds cap of {max_bytes} bytes"
            )));
        }
    }

    let initial_cap = response
        .content_length()
        .and_then(|len| usize::try_from(len).ok())
        .map_or(0, |len| len.min(max_bytes).min(INITIAL_BUFFER_CAP_BYTES));
    let mut buf = BytesMut::with_capacity(initial_cap);
    let mut stream = response.bytes_stream();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|err| {
            if err.is_timeout() {
                CoreError::UpstreamTimeout(format!("response stream timed out: {err}"))
            } else {
                CoreError::Upstream(format!("failed reading response stream: {err}"))
            }
        })?;

        if buf.len().saturating_add(chunk.len()) > max_bytes {
            return Err(CoreError::Upstream(format!(
                "response body exceeded cap of {max_bytes} bytes"
            )));
        }
        buf.extend_from_slice(&chunk);
    }

    Ok(buf.freeze())
}

/// Maps a non-success HTTP status code into a deterministic `CoreError`.
#[must_use]
pub fn parse_http_status_error(status: StatusCode, context: &str) -> CoreError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            CoreError::Unauthorized(format!("{context}: HTTP {status}"))
        }
        StatusCode::NOT_FOUND | StatusCode::GONE => {
            CoreError::NotFound(format!("{context}: HTTP {status}"))
        }
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            CoreError::InvalidInput(format!("{context}: HTTP {status}"))
        }
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => {
            CoreError::UpstreamTimeout(format!("{context}: HTTP {status}"))
        }
        _ => CoreError::Upstream(format!("{context}: HTTP {status}")),
    }
}

/// Returns `true` when an HTTP status represents a transient failure eligible for retry.
#[must_use]
pub fn is_transient_http_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}

/// Attaches `If-None-Match` and `If-Modified-Since` conditional headers when present.
pub fn apply_conditional_headers(
    mut request: reqwest::RequestBuilder,
    conditional: &ConditionalFetchHeaders,
) -> reqwest::RequestBuilder {
    if let Some(etag) = conditional
        .etag
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        request = request.header(HEADER_IF_NONE_MATCH, etag);
    }
    if let Some(last_modified) = conditional
        .last_modified
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        request = request.header(HEADER_IF_MODIFIED_SINCE, last_modified);
    }
    request
}

/// Extracts `ETag` and `Last-Modified` validators from an HTTP response header map.
#[must_use]
pub fn extract_conditional_headers(headers: &HeaderMap) -> ConditionalFetchHeaders {
    let etag = headers
        .get(HEADER_ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let last_modified = headers
        .get(HEADER_LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    ConditionalFetchHeaders {
        etag,
        last_modified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn read_response_bytes_capped_aborts_when_exceeding_limit() {
        let server = MockServer::start().await;
        let payload = vec![b'a'; 2048];
        Mock::given(method("GET"))
            .and(path("/large"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload))
            .mount(&server)
            .await;

        let client = shared_http_client(None, &CollectorTuning::default()).unwrap();
        let resp = client
            .get(format!("{}/large", server.uri()))
            .send()
            .await
            .unwrap();
        let err = read_response_bytes_capped(resp, 1024).await.unwrap_err();
        assert!(matches!(err, CoreError::Upstream(_)));
        assert!(err.to_string().contains("1024"));
    }

    #[tokio::test]
    async fn read_response_bytes_capped_aborts_chunked_stream_without_content_length() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut req_buf = [0u8; 1024];
                let _ = socket.read(&mut req_buf).await;
                let headers =
                    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                let _ = socket.write_all(headers.as_bytes()).await;
                let chunk1 = format!("{:x}\r\n{}\r\n", 700, "x".repeat(700));
                let chunk2 = format!("{:x}\r\n{}\r\n0\r\n\r\n", 700, "y".repeat(700));
                let _ = socket.write_all(chunk1.as_bytes()).await;
                let _ = socket.write_all(chunk2.as_bytes()).await;
            }
        });

        let client = shared_http_client(None, &CollectorTuning::default()).unwrap();
        let resp = client
            .get(format!("http://{addr}/stream"))
            .send()
            .await
            .unwrap();
        assert!(resp.content_length().is_none());
        let err = read_response_bytes_capped(resp, 1024).await.unwrap_err();
        assert!(matches!(err, CoreError::Upstream(_)));
        assert!(err.to_string().contains("1024"));
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn read_response_bytes_capped_reads_body_under_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/small"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello-digest"))
            .mount(&server)
            .await;

        let client = shared_http_client(None, &CollectorTuning::default()).unwrap();
        let resp = client
            .get(format!("{}/small", server.uri()))
            .send()
            .await
            .unwrap();
        let bytes = read_response_bytes_capped(resp, 1024).await.unwrap();
        assert_eq!(bytes.as_ref(), b"hello-digest");
    }

    #[test]
    fn shared_http_client_and_semaphore_reuse_singletons() {
        let tuning = CollectorTuning::default();
        let c1 = shared_http_client(None, &tuning).unwrap();
        let c2 = shared_http_client(Some("   "), &tuning).unwrap();
        // Verify proxy cache handles valid proxy URL and rejects invalid URL cleanly.
        let p1 = shared_http_client(Some("http://127.0.0.1:8888"), &tuning).unwrap();
        let p2 = shared_http_client(Some("http://127.0.0.1:8888"), &tuning).unwrap();
        let _ = (c1, c2, p1, p2);

        let bad = shared_http_client(Some("://not-a-valid-proxy"), &tuning);
        assert!(matches!(bad, Err(CoreError::InvalidConfig(_))));

        let s1 = global_fetch_semaphore(4);
        let s2 = global_fetch_semaphore(8);
        assert!(Arc::ptr_eq(&s1, &s2));
    }

    #[test]
    fn parse_http_status_error_maps_expected_variants() {
        assert!(matches!(
            parse_http_status_error(StatusCode::UNAUTHORIZED, "x"),
            CoreError::Unauthorized(_)
        ));
        assert!(matches!(
            parse_http_status_error(StatusCode::NOT_FOUND, "gh"),
            CoreError::NotFound(_)
        ));
        assert!(matches!(
            parse_http_status_error(StatusCode::GATEWAY_TIMEOUT, "rss"),
            CoreError::UpstreamTimeout(_)
        ));
        assert!(matches!(
            parse_http_status_error(StatusCode::TOO_MANY_REQUESTS, "hn"),
            CoreError::Upstream(_)
        ));
        assert!(is_transient_http_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_transient_http_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(!is_transient_http_status(StatusCode::NOT_FOUND));
    }
}
