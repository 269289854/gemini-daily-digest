//! Public OIDC config endpoint (`GET /api/v1/auth/config`) and current user profile
//! (`GET /api/v1/me`).

use axum::extract::State;
use axum::Json;
use digest_core::constants::{LOCALE_EN_US, LOCALE_ZH_CN};
use digest_core::domain::AuthMode;
use serde::{Deserialize, Serialize};

use crate::api::admin::compute_platform_x_ready;
use crate::api::error::{ApiError, ApiJson, RequestId};
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

/// Request payload for `PUT /api/v1/me/preferences`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserPreferencesRequest {
    pub preferred_locale: String,
}

/// `PUT /api/v1/me/preferences` — updates the authenticated user's preferred digest/UI locale.
pub async fn update_me_preferences(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiJson(body): ApiJson<UpdateUserPreferencesRequest>,
) -> Result<Json<UserRecord>, ApiError> {
    let trimmed = body.preferred_locale.trim();
    let normalized =
        if trimmed.eq_ignore_ascii_case(LOCALE_ZH_CN) || trimmed.eq_ignore_ascii_case("zh") {
            LOCALE_ZH_CN
        } else if trimmed.eq_ignore_ascii_case(LOCALE_EN_US) || trimmed.eq_ignore_ascii_case("en") {
            LOCALE_EN_US
        } else {
            return Err(ApiError::invalid_input(
                req_id.0,
                "preferredLocale must be 'zh-CN' or 'en-US'",
            ));
        };

    let updated = state
        .repos
        .users
        .update_user_locale(user.id, normalized)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?;

    Ok(Json(updated))
}
