//! Authentication service (Authentik OIDC RS256 JWKS validator and loopback dev bypass)
//! plus the Axum `AuthenticatedUser` request extractor.

pub mod dev_bypass;
pub mod oidc;

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{ConnectInfo, FromRequestParts};
use digest_core::domain::AuthMode;
use digest_core::error::CoreError;
use http::request::Parts;
use http::HeaderMap;

use crate::api::error::{ApiError, RequestId};
use crate::config::ServerConfig;
use crate::repo::{UserRecord, UserRepo};
use crate::services::AuthService;
use crate::state::AppState;

use self::dev_bypass::authenticate_dev_bypass;
use self::oidc::OidcValidator;

/// Default `AuthService` dispatching on `config.core.auth_mode` (`Oidc` vs `DevBypass`).
pub struct HybridAuthService {
    config: Arc<ServerConfig>,
    users: Arc<dyn UserRepo>,
    oidc: OidcValidator,
}

/// Alias matching standard naming across services.
pub type DefaultAuthService = HybridAuthService;

impl HybridAuthService {
    /// Creates a new `HybridAuthService`.
    #[must_use]
    pub fn new(config: Arc<ServerConfig>, users: Arc<dyn UserRepo>, http: reqwest::Client) -> Self {
        let oidc = OidcValidator::new(config.clone(), http);
        Self {
            config,
            users,
            oidc,
        }
    }
}

#[async_trait]
impl AuthService for HybridAuthService {
    async fn authenticate_bearer_or_bypass(
        &self,
        headers: &HeaderMap,
        peer_addr: Option<SocketAddr>,
    ) -> Result<UserRecord, CoreError> {
        match self.config.core.auth_mode {
            AuthMode::Oidc => self.oidc.authenticate(self.users.as_ref(), headers).await,
            AuthMode::DevBypass => authenticate_dev_bypass(self.users.as_ref(), peer_addr).await,
        }
    }
}

/// Axum request extractor authenticating the caller into a resolved `UserRecord`.
///
/// Reads optional `ConnectInfo<SocketAddr>` from request extensions so missing
/// socket metadata fails closed in `DevBypass` mode rather than panicking.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser(pub UserRecord);

impl FromRequestParts<AppState> for AuthenticatedUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let RequestId(request_id) = RequestId::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|err| match err {});

        let peer_addr = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0);

        let user = state
            .auth
            .authenticate_bearer_or_bypass(&parts.headers, peer_addr)
            .await
            .map_err(|err| ApiError::new(request_id, err))?;

        Ok(Self(user))
    }
}
