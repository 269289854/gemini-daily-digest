//! Authenticated HTTP client communicating with `digest-server` REST endpoints.

use std::fmt;
use std::time::Duration;

use digest_core::constants::{
    CONTENT_TYPE_JSON, DEFAULT_HTTP_TIMEOUT_SECS, HEADER_ACCEPT, HEADER_AUTHORIZATION,
    HEADER_BEARER_PREFIX, HEADER_CONTENT_TYPE, HEADER_REQUEST_ID,
};
use digest_core::domain::ErrorCode;
use digest_core::error::CoreError;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client, Method, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::config::{McpRuntimeConfig, McpSharedState};
use crate::redact::sanitize_error_text;

const ACCEPT_MARKDOWN_OR_JSON: &str = "text/markdown, text/plain;q=0.9, application/json;q=0.8";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrorEnvelope {
    code: ErrorCode,
    message: String,
    #[serde(default)]
    request_id: String,
}

/// HTTP client for `digest-server`, sharing runtime connection state with Desktop/CLI.
#[derive(Clone)]
pub struct DigestApiClient {
    http: Client,
    shared: McpSharedState,
}

impl fmt::Debug for DigestApiClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DigestApiClient")
            .field("shared", &self.shared)
            .finish()
    }
}

impl DigestApiClient {
    /// Builds a client backed by an existing `McpSharedState` (live-synced with Desktop settings).
    pub fn from_shared_state(shared: McpSharedState) -> Result<Self, CoreError> {
        let http = Client::builder()
            .use_rustls_tls()
            .gzip(true)
            .brotli(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|err| CoreError::Internal(format!("failed to build HTTP client: {err}")))?;

        Ok(Self { http, shared })
    }

    /// Convenience constructor from an explicit `server_base_url` and optional `api_token`.
    pub fn new(
        server_base_url: impl Into<String>,
        api_token: Option<String>,
    ) -> Result<Self, CoreError> {
        let cfg = McpRuntimeConfig {
            server_base_url: server_base_url.into(),
            api_token,
            ..McpRuntimeConfig::default()
        };
        let shared = McpSharedState::new(&cfg)?;
        Self::from_shared_state(shared)
    }

    /// Dynamically updates `server_base_url` and `api_token` (used by Desktop runtime sync).
    pub fn update_connection(
        &self,
        server_base_url: impl Into<String>,
        api_token: Option<String>,
    ) -> Result<(), CoreError> {
        self.shared.update_connection(server_base_url, api_token)
    }

    /// Dynamically updates only `api_token`.
    pub fn set_api_token(&self, api_token: Option<String>) {
        self.shared.set_api_token(api_token);
    }

    /// Sends a `GET` request with optional query parameters and decodes JSON.
    pub async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<Value, CoreError> {
        let resp = self
            .send_request(Method::GET, path, query, None, CONTENT_TYPE_JSON)
            .await?;
        decode_json_response(resp).await
    }

    /// Sends a `GET` request expecting plain text / Markdown.
    pub async fn get_text(&self, path: &str) -> Result<String, CoreError> {
        let resp = self
            .send_request(Method::GET, path, &[], None, ACCEPT_MARKDOWN_OR_JSON)
            .await?;
        let status = resp.status();
        let body = resp.text().await.map_err(|err| {
            let is_timeout = err.is_timeout();
            let safe = sanitize_error_text(&err.without_url().to_string());
            if is_timeout {
                CoreError::UpstreamTimeout(format!("timed out reading response: {safe}"))
            } else {
                CoreError::Upstream(format!("failed reading response body: {safe}"))
            }
        })?;
        if !status.is_success() {
            return Err(map_error_body(status, &body));
        }
        Ok(body)
    }

    /// Sends a `POST` request with optional JSON body and decodes JSON.
    pub async fn post_json(&self, path: &str, body: Option<&Value>) -> Result<Value, CoreError> {
        let resp = self
            .send_request(Method::POST, path, &[], body, CONTENT_TYPE_JSON)
            .await?;
        decode_json_response(resp).await
    }

    /// Sends a `PUT` request with JSON body and decodes JSON.
    pub async fn put_json(&self, path: &str, body: &Value) -> Result<Value, CoreError> {
        let resp = self
            .send_request(Method::PUT, path, &[], Some(body), CONTENT_TYPE_JSON)
            .await?;
        decode_json_response(resp).await
    }

    /// Sends a `DELETE` request expecting an empty 2xx response (e.g. `204 No Content`).
    pub async fn delete_empty(&self, path: &str) -> Result<(), CoreError> {
        let resp = self
            .send_request(Method::DELETE, path, &[], None, CONTENT_TYPE_JSON)
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        Err(map_error_body(status, &body))
    }

    async fn send_request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        json_body: Option<&Value>,
        accept_header: &str,
    ) -> Result<reqwest::Response, CoreError> {
        let base_url = self.shared.server_base_url();
        let normalized_path = if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("/{path}")
        };
        let target_url = format!("{base_url}{normalized_path}");

        let mut headers = HeaderMap::new();
        let accept_val = HeaderValue::from_str(accept_header)
            .map_err(|err| CoreError::Internal(format!("invalid accept header: {err}")))?;
        headers.insert(HEADER_ACCEPT, accept_val);

        let request_id = Uuid::new_v4().to_string();
        let req_id_val = HeaderValue::from_str(&request_id)
            .map_err(|err| CoreError::Internal(format!("invalid request id header: {err}")))?;
        headers.insert(HEADER_REQUEST_ID, req_id_val);

        if let Some(token) = self.shared.api_token() {
            let bearer = format!("{HEADER_BEARER_PREFIX}{token}");
            let auth_val = HeaderValue::from_str(&bearer).map_err(|_| {
                CoreError::InvalidConfig("api_token contains invalid HTTP header bytes".to_owned())
            })?;
            headers.insert(HEADER_AUTHORIZATION, auth_val);
        }

        let mut req = self.http.request(method, &target_url).headers(headers);
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(body) = json_body {
            req = req
                .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
                .json(body);
        }

        req.send().await.map_err(|err| {
            let is_timeout = err.is_timeout();
            let safe = sanitize_error_text(&err.without_url().to_string());
            if is_timeout {
                CoreError::UpstreamTimeout(format!("digest-server request timed out: {safe}"))
            } else {
                CoreError::Upstream(format!("digest-server request failed: {safe}"))
            }
        })
    }
}

async fn decode_json_response(resp: reqwest::Response) -> Result<Value, CoreError> {
    let status = resp.status();
    let body = resp.text().await.map_err(|err| {
        let is_timeout = err.is_timeout();
        let safe = sanitize_error_text(&err.without_url().to_string());
        if is_timeout {
            CoreError::UpstreamTimeout(format!("timed out reading response: {safe}"))
        } else {
            CoreError::Upstream(format!("failed reading response body: {safe}"))
        }
    })?;

    if !status.is_success() {
        return Err(map_error_body(status, &body));
    }

    serde_json::from_str::<Value>(&body)
        .map_err(|err| CoreError::Upstream(format!("digest-server returned invalid JSON: {err}")))
}

fn map_error_body(status: StatusCode, body: &str) -> CoreError {
    if let Ok(env) = serde_json::from_str::<ErrorEnvelope>(body) {
        let msg = sanitize_error_text(&env.message);
        let detail = if env.request_id.trim().is_empty() {
            msg
        } else {
            format!("{msg} (requestId: {})", env.request_id.trim())
        };
        return match env.code {
            ErrorCode::InvalidInput => CoreError::InvalidInput(detail),
            ErrorCode::Unauthorized => CoreError::Unauthorized(detail),
            ErrorCode::Forbidden => CoreError::Forbidden(detail),
            ErrorCode::NotFound => CoreError::NotFound(detail),
            ErrorCode::Conflict => CoreError::Conflict(detail),
            ErrorCode::SsrfBlocked => CoreError::SsrfBlocked(detail),
            ErrorCode::CryptoError => CoreError::Crypto(detail),
            ErrorCode::UpstreamTimeout => CoreError::UpstreamTimeout(detail),
            ErrorCode::UpstreamError => CoreError::Upstream(detail),
            ErrorCode::DatabaseError => CoreError::Database(detail),
            ErrorCode::InternalError => CoreError::Internal(detail),
        };
    }

    let safe_body = sanitize_error_text(body.trim());
    let summary = if safe_body.is_empty() {
        format!("digest-server returned HTTP {status}")
    } else {
        format!("digest-server returned HTTP {status}: {safe_body}")
    };

    match status {
        StatusCode::BAD_REQUEST => CoreError::InvalidInput(summary),
        StatusCode::UNAUTHORIZED => CoreError::Unauthorized(summary),
        StatusCode::FORBIDDEN => CoreError::Forbidden(summary),
        StatusCode::NOT_FOUND => CoreError::NotFound(summary),
        StatusCode::CONFLICT => CoreError::Conflict(summary),
        StatusCode::UNPROCESSABLE_ENTITY => CoreError::SsrfBlocked(summary),
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => {
            CoreError::UpstreamTimeout(summary)
        }
        _ => CoreError::Upstream(summary),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[tokio::test]
    async fn client_attaches_headers_maps_error_envelopes_and_updates_connection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/digests"))
            .and(header("authorization", "Bearer initial-oidc-tok"))
            .and(header("accept", CONTENT_TYPE_JSON))
            .and(header_exists("x-request-id"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [] })))
            .mount(&server)
            .await;

        let client =
            DigestApiClient::new(server.uri(), Some("initial-oidc-tok".to_owned())).unwrap();
        assert!(!format!("{client:?}").contains("initial-oidc-tok"));

        let res = client.get_json("/api/v1/digests", &[]).await.unwrap();
        assert_eq!(res["items"], json!([]));

        Mock::given(method("GET"))
            .and(path("/api/v1/digests/2026-01-02"))
            .and(header("authorization", "Bearer refreshed-oidc-tok"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "code": "NOT_FOUND",
                "message": "digest missing for date",
                "requestId": "req-42"
            })))
            .mount(&server)
            .await;

        client
            .update_connection(server.uri(), Some("refreshed-oidc-tok".to_owned()))
            .unwrap();
        let err = client
            .get_json("/api/v1/digests/2026-01-02", &[])
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), ErrorCode::NotFound);
        assert!(err.to_string().contains("req-42"));
    }
}
