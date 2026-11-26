//! Public OIDC config endpoint (`GET /api/v1/auth/config`) and current user profile
//! (`GET /api/v1/me`).

use axum::extract::State;
use axum::Json;
use digest_core::domain::AuthMode;
use serde::Serialize;

use crate::api::admin::compute_platform_x_ready;
use crate::api::error::{ApiError, RequestId};
use crate::auth::AuthenticatedUser;
use crate::repo::{OnboardingFlags, UserRecord};
use crate::state::AppState;

/// Public authentication configuration returned by `GET /api/v1/auth/config`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthConfigResponse {
    pub auth_mode: AuthMode,
    pub issuer_url: Option<String>,
    pub client_id: Option<String>,
    pub scopes: Vec<String>,
}

/// Authenticated user profile and onboarding completion flags (`GET /api/v1/me`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub user: UserRecord,
    pub onboarding: OnboardingFlags,
    pub platform_x_ready: bool,
}

/// `GET /api/v1/auth/config` — unauthenticated public OIDC configuration.
pub async fn auth_config(State(state): State<AppState>) -> Json<AuthConfigResponse> {
    Json(AuthConfigResponse {
        auth_mode: state.config.core.auth_mode,
        issuer_url: state.config.oidc_issuer_url.clone(),
        client_id: state.config.oidc_client_id.clone(),
        scopes: state.config.oidc_scopes(),
    })
}

/// `GET /api/v1/me` — returns authenticated user profile and onboarding flags.
pub async fn get_me(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
) -> Result<Json<MeResponse>, ApiError> {
    let onboarding = state
        .repos
        .users
        .get_onboarding_flags(user.id)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?;
    let platform_x_ready = compute_platform_x_ready(&state)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?;

    Ok(Json(MeResponse {
        user,
        onboarding,
        platform_x_ready,
    }))
}
