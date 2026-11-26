//! System browser OIDC PKCE loopback listener (`127.0.0.1:0`) and OIDC HTTP bridge.

use std::collections::HashMap;
use std::time::Duration;

use digest_core::constants::{DEFAULT_HTTP_TIMEOUT_SECS, DEFAULT_MCP_BIND_HOST};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};
use url::Url;

const CALLBACK_PATH: &str = "/callback";
const FAVICON_PATH: &str = "/favicon.ico";
const OIDC_LOOPBACK_TIMEOUT_SECS: u64 = 180;
const CONNECTION_READ_TIMEOUT_SECS: u64 = 5;
const MAX_HTTP_REQUEST_BYTES: usize = 16 * 1024;
const MAX_OIDC_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

const SUCCESS_HTML: &str = "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Sign-in Complete</title></head><body style=\"font-family:system-ui,sans-serif;background:#0f172a;color:#e2e8f0;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0;\"><div style=\"max-width:420px;padding:32px;border-radius:12px;background:#1e293b;border:1px solid #334155;text-align:center;\"><h1 style=\"margin:0 0 12px;font-size:20px;color:#f8fafc;\">Sign-in complete</h1><p style=\"margin:0;font-size:14px;color:#94a3b8;\">You may close this browser tab and return to Gemini Daily Digest.</p></div></body></html>";

const ERROR_HTML_PREFIX: &str = "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Sign-in Error</title></head><body style=\"font-family:system-ui,sans-serif;background:#0f172a;color:#fecaca;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0;\"><div style=\"max-width:460px;padding:32px;border-radius:12px;background:#1e293b;border:1px solid #7f1d1d;text-align:center;\"><h1 style=\"margin:0 0 12px;font-size:20px;color:#fef2f2;\">Sign-in could not be completed</h1><p style=\"margin:0;font-size:14px;color:#fca5a5;\">";
const ERROR_HTML_SUFFIX: &str = "</p></div></body></html>";

#[derive(Default)]
struct LoopbackSlot {
    pending_listener: Option<TcpListener>,
    active_cancel_tx: Option<oneshot::Sender<()>>,
}

/// Managed Tauri state for ephemeral `127.0.0.1:0` OIDC callbacks and CORS-free OIDC HTTP calls.
pub struct OidcLoopbackState {
    slot: Mutex<LoopbackSlot>,
    http_client: Client,
}

/// Response envelope returned to `TauriPlatformBridge.oidcFetch`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OidcHttpResponse {
    pub status: u16,
    pub body: String,
}

impl OidcLoopbackState {
    /// Initializes OIDC loopback state with a hardened `rustls` HTTP client.
    pub fn new() -> Result<Self, String> {
        let http_client = Client::builder()
            .use_rustls_tls()
            .gzip(true)
            .brotli(true)
            .timeout(Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|err| format!("failed to construct OIDC HTTP client: {err}"))?;

        Ok(Self {
            slot: Mutex::new(LoopbackSlot::default()),
            http_client,
        })
    }
}

/// Validates that a URL is strictly `http:` or `https:` with a non-empty host and no control chars.
pub fn validate_http_https_url(raw: &str) -> Result<Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err("URL must be non-empty and contain no control characters".to_owned());
    }
    let parsed = Url::parse(trimmed).map_err(|err| format!("invalid URL: {err}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "blocked unsafe URL scheme '{}': only http and https are allowed",
            parsed.scheme()
        ));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("URL must include a non-empty host".to_owned());
    }
    Ok(parsed)
}

/// Cancels any previous loopback listener and binds a fresh `127.0.0.1:0` listener.
pub async fn prepare_loopback_inner(state: &OidcLoopbackState) -> Result<String, String> {
    let listener = TcpListener::bind((DEFAULT_MCP_BIND_HOST, 0))
        .await
        .map_err(|err| format!("failed to bind OIDC loopback listener: {err}"))?;
    let port = listener
        .local_addr()
        .map_err(|err| format!("failed to inspect OIDC loopback address: {err}"))?
        .port();

    {
        let mut guard = state.slot.lock().await;
        if let Some(cancel_tx) = guard.active_cancel_tx.take() {
            let _ = cancel_tx.send(());
        }
        drop(guard.pending_listener.take());
        guard.pending_listener = Some(listener);
    }

    Ok(format!(
        "http://{DEFAULT_MCP_BIND_HOST}:{port}{CALLBACK_PATH}"
    ))
}

async fn take_prepared_listener(
    state: &OidcLoopbackState,
) -> Result<(TcpListener, oneshot::Receiver<()>), String> {
    let mut guard = state.slot.lock().await;
    if let Some(prev_cancel) = guard.active_cancel_tx.take() {
        let _ = prev_cancel.send(());
    }
    let listener = guard.pending_listener.take().ok_or_else(|| {
        "OIDC loopback listener is not prepared; call prepare_oidc_loopback first".to_owned()
    })?;
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    guard.active_cancel_tx = Some(cancel_tx);
    Ok((listener, cancel_rx))
}

async fn clear_active_cancel(state: &OidcLoopbackState) {
    let mut guard = state.slot.lock().await;
    guard.active_cancel_tx = None;
}

/// Awaits the OIDC callback on the already-prepared loopback listener.
pub async fn await_prepared_loopback_callback(
    state: &OidcLoopbackState,
    timeout_duration: Duration,
) -> Result<String, String> {
    let (listener, cancel_rx) = take_prepared_listener(state).await?;
    let result =
        tokio::time::timeout(timeout_duration, accept_callback_loop(listener, cancel_rx)).await;
    clear_active_cancel(state).await;

    match result {
        Ok(inner) => inner,
        Err(_) => Err("OIDC login timed out waiting for browser callback".to_owned()),
    }
}

async fn accept_callback_loop(
    listener: TcpListener,
    mut cancel_rx: oneshot::Receiver<()>,
) -> Result<String, String> {
    loop {
        tokio::select! {
            _ = &mut cancel_rx => {
                return Err("OIDC login cancelled by a newer login request".to_owned());
            }
            accepted = listener.accept() => {
                let (mut stream, _) = accepted
                    .map_err(|err| format!("failed accepting OIDC loopback connection: {err}"))?;
                if let Some(outcome) = handle_loopback_connection(&mut stream).await? {
                    return outcome;
                }
            }
        }
    }
}

async fn handle_loopback_connection(
    stream: &mut TcpStream,
) -> Result<Option<Result<String, String>>, String> {
    let raw_request = match tokio::time::timeout(
        Duration::from_secs(CONNECTION_READ_TIMEOUT_SECS),
        read_http_head(stream),
    )
    .await
    {
        Ok(Ok(head)) => head,
        Ok(Err(_)) | Err(_) => return Ok(None),
    };

    let Some(first_line) = raw_request.lines().next() else {
        let _ = write_http_response(stream, 400, "text/plain; charset=utf-8", "Bad Request").await;
        return Ok(None);
    };

    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");

    if !method.eq_ignore_ascii_case("GET") {
        let _ = write_http_response(
            stream,
            405,
            "text/plain; charset=utf-8",
            "Method Not Allowed",
        )
        .await;
        return Ok(None);
    }

    let parsed = match Url::parse(&format!("http://{DEFAULT_MCP_BIND_HOST}{target}")) {
        Ok(u) => u,
        Err(_) => {
            let _ =
                write_http_response(stream, 400, "text/plain; charset=utf-8", "Invalid URL").await;
            return Ok(None);
        }
    };

    if parsed.path() == FAVICON_PATH || parsed.path() != CALLBACK_PATH {
        let _ = write_http_response(stream, 404, "text/plain; charset=utf-8", "Not Found").await;
        return Ok(None);
    }

    let mut code: Option<String> = None;
    let mut state: Option<String> = None;
    let mut oauth_error: Option<String> = None;
    let mut oauth_error_desc: Option<String> = None;

    for (key, value) in parsed.query_pairs() {
        let trimmed = value.trim();
        match key.as_ref() {
            "code" if !trimmed.is_empty() => code = Some(trimmed.to_owned()),
            "state" if !trimmed.is_empty() => state = Some(trimmed.to_owned()),
            "error" if !trimmed.is_empty() => oauth_error = Some(trimmed.to_owned()),
            "error_description" if !trimmed.is_empty() => {
                oauth_error_desc = Some(trimmed.to_owned());
            }
            _ => {}
        }
    }

    if let Some(err_code) = oauth_error {
        let msg = match oauth_error_desc {
            Some(desc) => format!("{err_code}: {desc}"),
            None => err_code,
        };
        let html = format!(
            "{ERROR_HTML_PREFIX}{}{ERROR_HTML_SUFFIX}",
            escape_html(&msg)
        );
        let _ = write_http_response(stream, 200, "text/html; charset=utf-8", &html).await;
        return Ok(Some(Err(format!("OIDC authorization error: {msg}"))));
    }

    if code.is_none() || state.is_none() {
        let msg = "Missing authorization code or state parameter in callback.";
        let html = format!("{ERROR_HTML_PREFIX}{msg}{ERROR_HTML_SUFFIX}");
        let _ = write_http_response(stream, 200, "text/html; charset=utf-8", &html).await;
        return Ok(Some(Err(
            "OIDC callback missing required code or state parameter".to_owned(),
        )));
    }

    let _ = write_http_response(stream, 200, "text/html; charset=utf-8", SUCCESS_HTML).await;
    let query = parsed.query().unwrap_or("");
    Ok(Some(Ok(format!("?{query}"))))
}

async fn read_http_head(stream: &mut TcpStream) -> Result<String, std::io::Error> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        let remaining = MAX_HTTP_REQUEST_BYTES.saturating_sub(buf.len());
        let take = n.min(remaining);
        buf.extend_from_slice(&chunk[..take]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= MAX_HTTP_REQUEST_BYTES {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), std::io::Error> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    stream.shutdown().await
}

fn escape_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Executes a validated `GET` or `POST` OIDC HTTP request using `reqwest`.
pub async fn execute_oidc_http_request(
    client: &Client,
    url: &str,
    method: &str,
    headers: HashMap<String, String>,
    body: Option<String>,
) -> Result<OidcHttpResponse, String> {
    let parsed_url = validate_http_https_url(url)?;
    let normalized_method = method.trim().to_ascii_uppercase();
    let req_method = match normalized_method.as_str() {
        "GET" => Method::GET,
        "POST" => Method::POST,
        other => {
            return Err(format!(
                "unsupported OIDC HTTP method '{other}': only GET and POST are allowed"
            ));
        }
    };

    let mut header_map = HeaderMap::new();
    for (raw_name, raw_val) in headers {
        let lower = raw_name.trim().to_ascii_lowercase();
        if matches!(lower.as_str(), "host" | "content-length" | "connection") {
            continue;
        }
        let name = HeaderName::from_bytes(lower.as_bytes())
            .map_err(|err| format!("invalid OIDC request header name '{raw_name}': {err}"))?;
        let val = HeaderValue::from_str(raw_val.trim())
            .map_err(|err| format!("invalid OIDC request header value for '{raw_name}': {err}"))?;
        header_map.insert(name, val);
    }

    let mut builder = client.request(req_method, parsed_url).headers(header_map);
    if let Some(payload) = body {
        builder = builder.body(payload);
    }

    let resp = builder
        .send()
        .await
        .map_err(|err| format!("OIDC HTTP request failed: {}", err.without_url()))?;
    let status = resp.status().as_u16();
    let bytes = resp
        .bytes()
        .await
        .map_err(|err| format!("failed reading OIDC response body: {}", err.without_url()))?;
    if bytes.len() > MAX_OIDC_RESPONSE_BYTES {
        return Err("OIDC response body exceeded 2 MiB limit".to_owned());
    }
    let body_text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(OidcHttpResponse {
        status,
        body: body_text,
    })
}

/// Binds an ephemeral loopback callback listener on `127.0.0.1:0`.
#[tauri::command]
pub async fn prepare_oidc_loopback(state: State<'_, OidcLoopbackState>) -> Result<String, String> {
    prepare_loopback_inner(state.inner()).await
}

/// Opens the system browser to `authorizeUrl` and awaits the OIDC callback on loopback.
#[tauri::command]
pub async fn open_oidc_system_login(
    app: AppHandle,
    state: State<'_, OidcLoopbackState>,
    authorize_url: String,
) -> Result<String, String> {
    let parsed = validate_http_https_url(&authorize_url)?;
    let (listener, cancel_rx) = take_prepared_listener(state.inner()).await?;

    if let Err(err) = app.opener().open_url(parsed.as_str(), None::<&str>) {
        clear_active_cancel(state.inner()).await;
        return Err(format!(
            "failed to open system browser for OIDC login: {err}"
        ));
    }

    let result = tokio::time::timeout(
        Duration::from_secs(OIDC_LOOPBACK_TIMEOUT_SECS),
        accept_callback_loop(listener, cancel_rx),
    )
    .await;
    clear_active_cancel(state.inner()).await;

    match result {
        Ok(inner) => inner,
        Err(_) => Err("OIDC login timed out waiting for browser callback".to_owned()),
    }
}

/// Performs an OIDC HTTP request (`GET` or `POST`) outside WebView2 CORS restrictions.
#[tauri::command]
pub async fn oidc_http_request(
    state: State<'_, OidcLoopbackState>,
    url: String,
    method: String,
    headers: HashMap<String, String>,
    body: Option<String>,
) -> Result<OidcHttpResponse, String> {
    execute_oidc_http_request(&state.http_client, &url, &method, headers, body).await
}

/// Opens a validated `http:` or `https:` URL in the system default browser.
#[tauri::command]
pub async fn open_external_url(app: AppHandle, url: String) -> Result<(), String> {
    let parsed = validate_http_https_url(&url)?;
    app.opener()
        .open_url(parsed.as_str(), None::<&str>)
        .map_err(|err| format!("failed to open external URL: {err}"))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;

    async fn send_raw_http_get(addr: &str, path_and_query: &str) -> String {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let req =
            format!("GET {path_and_query} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).await.unwrap();
        String::from_utf8_lossy(&resp).into_owned()
    }

    #[tokio::test]
    async fn prepare_oidc_loopback_binds_loopback_and_captures_callback_ignoring_favicon() {
        let state = OidcLoopbackState::new().unwrap();
        let first_uri = prepare_loopback_inner(&state).await.unwrap();
        assert!(first_uri.starts_with("http://127.0.0.1:"));
        assert!(first_uri.ends_with("/callback"));

        // Second prepare cancels/replaces the first prepared listener cleanly.
        let second_uri = prepare_loopback_inner(&state).await.unwrap();
        assert!(second_uri.starts_with("http://127.0.0.1:"));
        assert!(second_uri.ends_with("/callback"));
        let parsed = Url::parse(&second_uri).unwrap();
        let addr = format!("127.0.0.1:{}", parsed.port().unwrap());

        let waiter = tokio::spawn({
            let state_ref = &state;
            let _ = state_ref;
            async move {
                // Use direct take_prepared_listener + accept loop on `state` below.
            }
        });
        let _ = waiter.await;

        let addr_clone = addr.clone();
        let client_task = tokio::spawn(async move {
            let fav_resp = send_raw_http_get(&addr_clone, "/favicon.ico").await;
            assert!(fav_resp.starts_with("HTTP/1.1 404"));

            let cb_resp =
                send_raw_http_get(&addr_clone, "/callback?code=auth123&state=st456").await;
            assert!(cb_resp.starts_with("HTTP/1.1 200 OK"));
            assert!(cb_resp.contains("Sign-in complete"));
        });

        let query = await_prepared_loopback_callback(&state, Duration::from_secs(5))
            .await
            .unwrap();
        client_task.await.unwrap();
        assert_eq!(query, "?code=auth123&state=st456");
    }

    #[tokio::test]
    async fn callback_rejects_error_or_missing_params_and_url_method_validators_block_unsafe_inputs(
    ) {
        let state = OidcLoopbackState::new().unwrap();
        let redirect_uri = prepare_loopback_inner(&state).await.unwrap();
        let parsed = Url::parse(&redirect_uri).unwrap();
        let addr = format!("127.0.0.1:{}", parsed.port().unwrap());

        let client_task = tokio::spawn(async move {
            let resp = send_raw_http_get(
                &addr,
                "/callback?error=access_denied&error_description=Cancelled",
            )
            .await;
            assert!(resp.starts_with("HTTP/1.1 200 OK"));
            assert!(resp.contains("access_denied"));
        });

        let err = await_prepared_loopback_callback(&state, Duration::from_secs(5))
            .await
            .unwrap_err();
        client_task.await.unwrap();
        assert!(err.contains("access_denied"));

        // Missing code/state
        let redirect_uri_2 = prepare_loopback_inner(&state).await.unwrap();
        let parsed_2 = Url::parse(&redirect_uri_2).unwrap();
        let addr_2 = format!("127.0.0.1:{}", parsed_2.port().unwrap());
        let client_task_2 = tokio::spawn(async move {
            let _ = send_raw_http_get(&addr_2, "/callback?state=only_state").await;
        });
        let err_2 = await_prepared_loopback_callback(&state, Duration::from_secs(5))
            .await
            .unwrap_err();
        client_task_2.await.unwrap();
        assert!(err_2.contains("missing required code or state"));

        // Scheme & method validation
        assert!(validate_http_https_url("javascript:alert(1)").is_err());
        assert!(validate_http_https_url("file:///C:/Windows/System32").is_err());
        assert!(validate_http_https_url("https://auth.example.com/authorize").is_ok());

        let http = Client::new();
        let bad_scheme = execute_oidc_http_request(
            &http,
            "ftp://auth.example.com/token",
            "GET",
            HashMap::new(),
            None,
        )
        .await
        .unwrap_err();
        assert!(bad_scheme.contains("only http and https"));

        let bad_method = execute_oidc_http_request(
            &http,
            "https://auth.example.com/token",
            "DELETE",
            HashMap::new(),
            None,
        )
        .await
        .unwrap_err();
        assert!(bad_method.contains("only GET and POST"));
    }

    #[tokio::test]
    async fn second_prepare_cancels_in_flight_callback_and_oidc_http_request_handles_get_and_post()
    {
        let state = std::sync::Arc::new(OidcLoopbackState::new().unwrap());
        let _uri1 = prepare_loopback_inner(&state).await.unwrap();

        let state_waiter = std::sync::Arc::clone(&state);
        let waiter_task = tokio::spawn(async move {
            await_prepared_loopback_callback(&state_waiter, Duration::from_secs(10)).await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let _uri2 = prepare_loopback_inner(&state).await.unwrap();
        let cancel_err = waiter_task.await.unwrap().unwrap_err();
        assert!(cancel_err.contains("cancelled"));

        // Verify local GET and POST through execute_oidc_http_request
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_task = tokio::spawn(async move {
            for expected in ["GET /oidc-meta", "POST /token"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let req = read_http_head(&mut stream).await.unwrap();
                assert!(req.starts_with(expected));
                let body = r#"{"ok":true}"#;
                write_http_response(&mut stream, 200, "application/json", body)
                    .await
                    .unwrap();
            }
        });

        let get_res = execute_oidc_http_request(
            &state.http_client,
            &format!("http://127.0.0.1:{port}/oidc-meta"),
            "GET",
            HashMap::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(get_res.status, 200);
        assert_eq!(get_res.body, r#"{"ok":true}"#);

        let mut headers = HashMap::new();
        headers.insert(
            "content-type".to_owned(),
            "application/x-www-form-urlencoded".to_owned(),
        );
        let post_res = execute_oidc_http_request(
            &state.http_client,
            &format!("http://127.0.0.1:{port}/token"),
            "POST",
            headers,
            Some("grant_type=authorization_code&code=abc".to_owned()),
        )
        .await
        .unwrap();
        assert_eq!(post_res.status, 200);
        assert_eq!(post_res.body, r#"{"ok":true}"#);

        server_task.await.unwrap();
    }
}
