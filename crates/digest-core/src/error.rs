//! Structured error definitions mapped deterministically to `ErrorCode`.

use thiserror::Error;

use crate::domain::ErrorCode;

/// Core domain and infrastructure error type.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid {enum_name} value: {value}")]
    InvalidEnumValue {
        enum_name: &'static str,
        value: String,
    },

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("resource not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("blocked by SSRF policy: {0}")]
    SsrfBlocked(String),

    #[error("cryptographic failure: {0}")]
    Crypto(String),

    #[error("upstream request timed out: {0}")]
    UpstreamTimeout(String),

    #[error("upstream error: {0}")]
    Upstream(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl CoreError {
    /// Maps the error variant to its canonical protocol `ErrorCode`.
    #[must_use]
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            Self::InvalidEnumValue { .. } | Self::InvalidConfig(_) | Self::InvalidInput(_) => {
                ErrorCode::InvalidInput
            }
            Self::Unauthorized(_) => ErrorCode::Unauthorized,
            Self::Forbidden(_) => ErrorCode::Forbidden,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::SsrfBlocked(_) => ErrorCode::SsrfBlocked,
            Self::Crypto(_) => ErrorCode::CryptoError,
            Self::UpstreamTimeout(_) => ErrorCode::UpstreamTimeout,
            Self::Upstream(_) => ErrorCode::UpstreamError,
            Self::Database(_) => ErrorCode::DatabaseError,
            Self::Internal(_) => ErrorCode::InternalError,
        }
    }
}
