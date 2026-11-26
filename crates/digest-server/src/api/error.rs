//! Unified HTTP error envelope (`{ code, message, requestId }`),
//! `x-request-id` extractor, and JSON/path parsing helpers.

use std::convert::Infallible;

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use chrono::NaiveDate;
use digest_core::constants::{CONTENT_TYPE_JSON, HEADER_REQUEST_ID};
use digest_core::domain::ErrorCode;
use digest_core::error::CoreError;
use http::header::HeaderName;
use http::request::Parts;
use http::{HeaderMap, HeaderValue, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use uuid::Uuid;

/// Machine-readable error JSON envelope matching `docs/API_CONTRACT.md` Section 4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorEnvelope {
    pub code: ErrorCode,
    pub message: String,
    pub request_id: String,
}

/// Request ID extracted from `x-request-id` header/extension, or generated as UUIDv4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(pub String);

impl RequestId {
    /// Extracts the `x-request-id` value from request headers or falls back to a fresh UUIDv4.
    #[must_use]
    pub fn from_headers(headers: &HeaderMap) -> Self {
        if let Some(raw) = headers
            .get(HEADER_REQUEST_ID)
            .and_then(|val| val.to_str().ok())
            .map(str::trim)
            .filter(|val| !val.is_empty())
        {
            return Self(raw.to_owned());
        }
        Self(Uuid::new_v4().to_string())
    }
}

impl<S> FromRequestParts<S> for RequestId
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(ext) = parts.extensions.get::<tower_http::request_id::RequestId>() {
            if let Ok(raw) = ext.header_value().to_str() {
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    return Ok(Self(trimmed.to_owned()));
                }
            }
        }
        Ok(Self::from_headers(&parts.headers))
    }
}

/// Unified API error converting `CoreError` into an HTTP status and JSON envelope.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: ErrorCode,
    pub message: String,
    pub request_id: String,
}

impl ApiError {
    /// Constructs an `ApiError` from a `RequestId` and `CoreError`, sanitizing internal 500s.
    #[must_use]
    pub fn new(request_id: impl Into<String>, err: CoreError) -> Self {
        let request_id = request_id.into();
        let code = err.error_code();
        let (status, message) = map_core_error(&request_id, &err);
        Self {
            status,
            code,
            message,
            request_id,
        }
    }

    /// Convenience helper building an `INVALID_INPUT` 400 error.
    #[must_use]
    pub fn invalid_input(request_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(request_id, CoreError::InvalidInput(message.into()))
    }
}

impl From<CoreError> for ApiError {
    fn from(err: CoreError) -> Self {
        Self::new(Uuid::new_v4().to_string(), err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let envelope = ErrorEnvelope {
            code: self.code,
            message: self.message,
            request_id: self.request_id.clone(),
        };
        let mut response = (self.status, Json(envelope)).into_response();
        if let Ok(val) = HeaderValue::from_str(&self.request_id) {
            response
                .headers_mut()
                .insert(HeaderName::from_static(HEADER_REQUEST_ID), val);
        }
        response
    }
}

fn map_core_error(request_id: &str, err: &CoreError) -> (StatusCode, String) {
    match err {
        CoreError::InvalidEnumValue { .. }
        | CoreError::InvalidConfig(_)
        | CoreError::InvalidInput(_) => (StatusCode::BAD_REQUEST, err.to_string()),
        CoreError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, err.to_string()),
        CoreError::Forbidden(_) => (StatusCode::FORBIDDEN, err.to_string()),
        CoreError::NotFound(_) => (StatusCode::NOT_FOUND, err.to_string()),
        CoreError::Conflict(_) => (StatusCode::CONFLICT, err.to_string()),
        CoreError::SsrfBlocked(_) => (StatusCode::UNPROCESSABLE_ENTITY, err.to_string()),
        CoreError::UpstreamTimeout(_) => (StatusCode::GATEWAY_TIMEOUT, err.to_string()),
        CoreError::Upstream(_) => (StatusCode::BAD_GATEWAY, err.to_string()),
        CoreError::Crypto(_) => {
            tracing::error!(
                request_id = %request_id,
                code = %ErrorCode::CryptoError,
                "cryptographic failure while handling request"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Cryptographic operation failed".to_owned(),
            )
        }
        CoreError::Database(_) => {
            tracing::error!(
                request_id = %request_id,
                code = %ErrorCode::DatabaseError,
                "database failure while handling request"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database operation failed".to_owned(),
            )
        }
        CoreError::Internal(_) => {
            tracing::error!(
                request_id = %request_id,
                code = %ErrorCode::InternalError,
                "internal server error while handling request"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_owned(),
            )
        }
    }
}

/// JSON body extractor that converts malformed payloads into a structured `ApiError`.
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let request_id = RequestId::from_headers(req.headers()).0;
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|err| ApiError::invalid_input(&request_id, format!("invalid body: {err}")))?;
        let parsed = serde_json::from_slice::<T>(&bytes).map_err(|err| {
            ApiError::invalid_input(&request_id, format!("invalid JSON payload: {err}"))
        })?;
        Ok(Self(parsed))
    }
}

/// Query string extractor that converts malformed query params into a structured `ApiError`.
pub struct ApiQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let RequestId(request_id) = RequestId::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|err| match err {});
        let axum::extract::Query(value) =
            axum::extract::Query::<T>::from_request_parts(parts, state)
                .await
                .map_err(|err| {
                    ApiError::invalid_input(&request_id, format!("invalid query parameters: {err}"))
                })?;
        Ok(Self(value))
    }
}

/// Optional JSON body extractor accepting empty bodies as `T::default()`.
pub struct OptionalApiJson<T>(pub T);

impl<S, T> FromRequest<S> for OptionalApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Default,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let request_id = RequestId::from_headers(req.headers()).0;
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|err| ApiError::invalid_input(&request_id, format!("invalid body: {err}")))?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self(T::default()));
        }
        let parsed = serde_json::from_slice::<T>(&bytes).map_err(|err| {
            ApiError::invalid_input(&request_id, format!("invalid JSON payload: {err}"))
        })?;
        Ok(Self(parsed))
    }
}

/// Helper to parse a `Uuid` path/query parameter into `CoreError::InvalidInput`.
pub fn parse_uuid(raw: &str, field: &str) -> Result<Uuid, CoreError> {
    Uuid::parse_str(raw.trim())
        .map_err(|_| CoreError::InvalidInput(format!("invalid UUID for {field}")))
}

/// Helper to parse a `YYYY-MM-DD` date parameter into `CoreError::InvalidInput`.
pub fn parse_date(raw: &str, field: &str) -> Result<NaiveDate, CoreError> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
        .map_err(|_| CoreError::InvalidInput(format!("{field} must be in YYYY-MM-DD format")))
}

/// Returns JSON content-type helper constant check for tests/headers.
#[must_use]
pub const fn json_content_type() -> &'static str {
    CONTENT_TYPE_JSON
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http::Request;
    use http_body_util::BodyExt;
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Default, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SamplePayload {
        flag: bool,
    }

    #[tokio::test]
    async fn api_error_maps_all_core_errors_and_sanitizes_500_messages() {
        let cases = [
            (
                CoreError::InvalidInput("bad field".to_owned()),
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidInput,
            ),
            (
                CoreError::Unauthorized("missing token".to_owned()),
                StatusCode::UNAUTHORIZED,
                ErrorCode::Unauthorized,
            ),
            (
                CoreError::Forbidden("loopback only".to_owned()),
                StatusCode::FORBIDDEN,
                ErrorCode::Forbidden,
            ),
            (
                CoreError::NotFound("digest missing".to_owned()),
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
            ),
            (
                CoreError::Conflict("already exists".to_owned()),
                StatusCode::CONFLICT,
                ErrorCode::Conflict,
            ),
            (
                CoreError::SsrfBlocked("private ip".to_owned()),
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::SsrfBlocked,
            ),
            (
                CoreError::UpstreamTimeout("timed out".to_owned()),
                StatusCode::GATEWAY_TIMEOUT,
                ErrorCode::UpstreamTimeout,
            ),
            (
                CoreError::Upstream("bad gateway".to_owned()),
                StatusCode::BAD_GATEWAY,
                ErrorCode::UpstreamError,
            ),
            (
                CoreError::Crypto("v1:secret-ciphertext-leak".to_owned()),
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::CryptoError,
            ),
            (
                CoreError::Database("SELECT * FROM users WHERE secret='abc'".to_owned()),
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::DatabaseError,
            ),
            (
                CoreError::Internal("internal secret-key-leak".to_owned()),
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::InternalError,
            ),
        ];

        for (err, expected_status, expected_code) in cases {
            let api_err = ApiError::new("req-123", err);
            assert_eq!(api_err.status, expected_status);
            assert_eq!(api_err.code, expected_code);

            let resp = api_err.into_response();
            assert_eq!(resp.status(), expected_status);
            assert_eq!(
                resp.headers()
                    .get(HEADER_REQUEST_ID)
                    .and_then(|v| v.to_str().ok()),
                Some("req-123")
            );

            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(json["code"], expected_code.as_str());
            assert_eq!(json["requestId"], "req-123");
            let msg = json["message"].as_str().unwrap();
            assert!(!msg.contains("SELECT * FROM"));
            assert!(!msg.contains("v1:secret-ciphertext-leak"));
            assert!(!msg.contains("internal secret-key-leak"));
        }
    }

    #[tokio::test]
    async fn request_id_extracts_header_or_generates_uuid() {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_REQUEST_ID, HeaderValue::from_static("custom-req-99"));
        assert_eq!(RequestId::from_headers(&headers).0, "custom-req-99");

        let empty_headers = HeaderMap::new();
        let generated = RequestId::from_headers(&empty_headers);
        assert!(Uuid::parse_str(&generated.0).is_ok());
    }

    #[tokio::test]
    async fn api_json_and_optional_api_json_validate_payloads_with_request_id() {
        let req_empty = Request::builder()
            .header(HEADER_REQUEST_ID, "req-opt-1")
            .body(Body::empty())
            .unwrap();
        let OptionalApiJson(parsed_default) =
            OptionalApiJson::<SamplePayload>::from_request(req_empty, &())
                .await
                .unwrap();
        assert!(!parsed_default.flag);

        let req_bad = Request::builder()
            .header(HEADER_REQUEST_ID, "req-bad-json")
            .body(Body::from("{not-valid"))
            .unwrap();
        let err = ApiJson::<SamplePayload>::from_request(req_bad, &())
            .await
            .err()
            .unwrap();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert_eq!(err.request_id, "req-bad-json");
    }
}
