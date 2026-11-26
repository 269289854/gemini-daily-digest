//! Administrator platform integration endpoints (`/api/v1/admin/platform`),
//! encrypted credential storage, and zero-restart bridge hot-reload dispatch.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::Json;
use digest_core::constants::{
    HEADER_AUTHORIZATION, HEADER_BEARER_PREFIX, SECRET_MASK_CONFIGURED_PLACEHOLDER,
    SECRET_MASK_REDACTED_PLACEHOLDER,
};
use digest_core::crypto::mask_secret;
use digest_core::domain::{FetchOutcome, SourceScope, SourceType};
use digest_core::error::CoreError;
use digest_core::traits::SourceDescriptor;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::error::{ApiError, ApiJson, OptionalApiJson, RequestId};
use crate::auth::AuthenticatedUser;
use crate::config::ServerConfig;
use crate::constants::{
    ADMIN_USER_ROLE, BRIDGE_RELOAD_HTTP_TIMEOUT_SECS, SOURCE_TEST_SAMPLE_TITLE_LIMIT,
};
use crate::repo::{
    PlatformSettingsRecord, PlatformSettingsView, UpsertPlatformSettingsRecord, UserRecord,
};
use crate::scheduler::poller::{
    fetch_with_platform_x_support, sanitize_x_platform_error, EffectivePlatformXConfig,
};
use crate::state::AppState;

const DEFAULT_X_TEST_HANDLE: &str = "GoogleDeepMind";
const MODE_BRIDGE: &str = "rsshub_bridge";
const MODE_OFFICIAL_API: &str = "official_api";
const MODE_UNCONFIGURED: &str = "unconfigured";

/// Request payload for `PUT /api/v1/admin/platform` (redacted `Debug`).
#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlatformSettingsRequest {
    #[serde(default)]
    pub x_bridge_base_url: Option<String>,
    #[serde(default)]
    pub x_bridge_reload_url: Option<String>,
    #[serde(default)]
    pub x_bridge_reload_secret: Option<String>,
    #[serde(default)]
    pub clear_x_bridge_reload_secret: bool,
    #[serde(default)]
    pub x_auth_token: Option<String>,
    #[serde(default)]
    pub clear_x_auth_token: bool,
    #[serde(default)]
    pub x_bearer_token: Option<String>,
    #[serde(default)]
    pub clear_x_bearer_token: bool,
}

impl fmt::Debug for UpdatePlatformSettingsRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redact = |opt: &Option<String>| {
            opt.as_ref()
                .filter(|s| !s.trim().is_empty())
                .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER)
        };
        f.debug_struct("UpdatePlatformSettingsRequest")
            .field("x_bridge_base_url", &self.x_bridge_base_url)
            .field("x_bridge_reload_url", &self.x_bridge_reload_url)
            .field(
                "x_bridge_reload_secret",
                &redact(&self.x_bridge_reload_secret),
            )
            .field(
                "clear_x_bridge_reload_secret",
                &self.clear_x_bridge_reload_secret,
            )
            .field("x_auth_token", &redact(&self.x_auth_token))
            .field("clear_x_auth_token", &self.clear_x_auth_token)
            .field("x_bearer_token", &redact(&self.x_bearer_token))
            .field("clear_x_bearer_token", &self.clear_x_bearer_token)
            .finish()
    }
}

/// Response payload for `PUT /api/v1/admin/platform`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlatformSettingsResponse {
    pub settings: PlatformSettingsView,
    pub bridge_reloaded: bool,
    pub bridge_message: Option<String>,
}

/// Optional request body for `POST /api/v1/admin/platform/x/test`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformXTestRequest {
    #[serde(default)]
    pub handle: Option<String>,
}

/// Response payload for `POST /api/v1/admin/platform/x/test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformXTestResult {
    pub ok: bool,
    pub mode: String,
    pub handle: String,
    pub items_fetched: usize,
    pub sample_titles: Vec<String>,
    pub latency_ms: u64,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeReloadPayload<'a> {
    auth_token: &'a str,
}

fn require_admin(user: &UserRecord, req_id: &str) -> Result<(), ApiError> {
    if user.role.trim().eq_ignore_ascii_case(ADMIN_USER_ROLE) {
        Ok(())
    } else {
        Err(ApiError::new(
            req_id,
            CoreError::Forbidden("administrator role is required".to_owned()),
        ))
    }
}

fn non_empty_trimmed(val: Option<&str>) -> Option<String> {
    val.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Extracts and normalizes one or more X `auth_token` values.
///
/// Accepts either:
/// - a single raw token (`a0a359...`)
/// - comma-separated tokens (`tok1, tok2`)
/// - a full browser `Cookie` header or copied header block containing `auth_token=...;`
pub fn extract_x_auth_token(raw: &str) -> Result<String, CoreError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(
            "X auth_token must not be empty".to_owned(),
        ));
    }

    if let Some(pos) = trimmed.find("auth_token=") {
        let after = &trimmed[pos + "auth_token=".len()..];
        let token_raw = after
            .split([';', '\r', '\n', '\0'])
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .trim();
        let token = token_raw
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim()
            .to_owned();
        if token.is_empty() {
            return Err(CoreError::InvalidInput(
                "found auth_token= prefix without a valid token value".to_owned(),
            ));
        }
        return Ok(token);
    }

    if trimmed.chars().any(|c| c == '\r' || c == '\n' || c == '\0') {
        return Err(CoreError::InvalidInput(
            "X auth_token must not contain newline or control characters".to_owned(),
        ));
    }

    let tokens: Vec<&str> = trimmed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if tokens.is_empty() {
        return Err(CoreError::InvalidInput(
            "X auth_token must not be empty".to_owned(),
        ));
    }
    Ok(tokens.join(","))
}

/// Resolves the effective platform X configuration by merging DB row overrides with `ServerConfig`.
#[must_use]
pub fn resolve_platform_view(
    record: Option<&PlatformSettingsRecord>,
    config: &ServerConfig,
) -> PlatformSettingsView {
    let x_bridge_base_url = record
        .and_then(|r| non_empty_trimmed(r.x_bridge_base_url.as_deref()))
        .or_else(|| non_empty_trimmed(config.platform_x_bridge_base_url.as_deref()));
    let x_bridge_reload_url = record
        .and_then(|r| non_empty_trimmed(r.x_bridge_reload_url.as_deref()))
        .or_else(|| non_empty_trimmed(config.platform_x_bridge_reload_url.as_deref()));

    let db_has_reload_secret = record
        .and_then(|r| r.x_bridge_reload_secret_enc.as_deref())
        .is_some_and(|s| !s.trim().is_empty());
    let env_has_reload_secret = config
        .platform_x_bridge_reload_secret
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    let has_x_bridge_reload_secret = db_has_reload_secret || env_has_reload_secret;
    let x_bridge_reload_secret_hint = if db_has_reload_secret {
        record
            .and_then(|r| r.x_bridge_reload_secret_hint.clone())
            .or_else(|| Some(SECRET_MASK_CONFIGURED_PLACEHOLDER.to_owned()))
    } else if env_has_reload_secret {
        Some(SECRET_MASK_CONFIGURED_PLACEHOLDER.to_owned())
    } else {
        None
    };

    let has_x_auth_token = record
        .and_then(|r| r.x_auth_token_enc.as_deref())
        .is_some_and(|s| !s.trim().is_empty());
    let x_auth_token_hint = if has_x_auth_token {
        record.and_then(|r| r.x_auth_token_hint.clone())
    } else {
        None
    };

    let has_x_bearer_token = record
        .and_then(|r| r.x_bearer_token_enc.as_deref())
        .is_some_and(|s| !s.trim().is_empty());
    let x_bearer_token_hint = if has_x_bearer_token {
        record.and_then(|r| r.x_bearer_token_hint.clone())
    } else {
        None
    };

    let x_bridge_configured = x_bridge_base_url.is_some();
    let x_integration_ready = x_bridge_configured || has_x_bearer_token;

    PlatformSettingsView {
        x_bridge_base_url,
        x_bridge_reload_url,
        has_x_bridge_reload_secret,
        x_bridge_reload_secret_hint,
        has_x_auth_token,
        x_auth_token_hint,
        has_x_bearer_token,
        x_bearer_token_hint,
        x_bridge_configured,
        x_integration_ready,
        updated_at: record.and_then(|r| r.updated_at),
    }
}

fn validate_optional_http_url(
    raw: Option<&str>,
    field_label: &str,
) -> Result<Option<String>, CoreError> {
    let Some(trimmed) = non_empty_trimmed(raw) else {
        return Ok(None);
    };
    let parsed = url::Url::parse(&trimmed).map_err(|err| {
        CoreError::InvalidInput(format!("{field_label} must be a valid URL: {err}"))
    })?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(CoreError::InvalidInput(format!(
            "{field_label} must use http or https scheme"
        )));
    }
    Ok(Some(trimmed.trim_end_matches('/').to_owned()))
}

async fn resolve_secret_update(
    cipher: Arc<digest_core::crypto::EnvelopeCipher>,
    new_plain: Option<String>,
    clear_flag: bool,
    existing_enc: Option<String>,
    existing_hint: Option<String>,
) -> Result<(Option<String>, Option<String>), CoreError> {
    if clear_flag {
        return Ok((None, None));
    }
    if let Some(secret) = new_plain {
        let trimmed = secret.trim().to_owned();
        if trimmed.is_empty() {
            return Ok((existing_enc, existing_hint));
        }
        tokio::task::spawn_blocking(move || {
            let enc = cipher.encrypt(&trimmed)?;
            let hint = mask_secret(&trimmed);
            Ok((Some(enc), Some(hint)))
        })
        .await
        .map_err(|err| CoreError::Internal(format!("secret encryption task failed: {err}")))?
    } else {
        Ok((existing_enc, existing_hint))
    }
}

async fn push_bridge_token_reload(
    state: &AppState,
    reload_url: &str,
    auth_token: &str,
    reload_secret: Option<&str>,
) -> Result<(), String> {
    if let Err(err) = state.ssrf.validate_url(reload_url).await {
        return Err(format!("bridge reload URL blocked by SSRF guard: {err}"));
    }

    let mut req = state
        .http
        .post(reload_url)
        .timeout(Duration::from_secs(BRIDGE_RELOAD_HTTP_TIMEOUT_SECS))
        .json(&BridgeReloadPayload { auth_token });
    if let Some(secret) = reload_secret.map(str::trim).filter(|s| !s.is_empty()) {
        req = req.header(
            HEADER_AUTHORIZATION,
            format!("{HEADER_BEARER_PREFIX}{secret}"),
        );
    }

    let resp = req
        .send()
        .await
        .map_err(|err| format!("bridge reload request failed: {err}"))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body_snippet = resp
        .text()
        .await
        .ok()
        .map(|s| s.trim().chars().take(180).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
    Err(format!(
        "bridge reload endpoint returned HTTP {}: {}",
        status.as_u16(),
        sanitize_x_platform_error(
            &body_snippet,
            reload_secret.into_iter().chain(std::iter::once(auth_token))
        )
    ))
}

/// `GET /api/v1/admin/platform` — returns non-secret platform integration status for admins.
pub async fn get_platform_settings(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
) -> Result<Json<PlatformSettingsView>, ApiError> {
    require_admin(&user, &req_id.0)?;
    let stored = state
        .repos
        .settings
        .get_platform_settings()
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;
    Ok(Json(resolve_platform_view(stored.as_ref(), &state.config)))
}

/// `PUT /api/v1/admin/platform` — updates platform X credentials/URLs and hot-reloads bridge.
pub async fn update_platform_settings(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiJson(body): ApiJson<UpdatePlatformSettingsRequest>,
) -> Result<Json<UpdatePlatformSettingsResponse>, ApiError> {
    require_admin(&user, &req_id.0)?;

    let existing = state
        .repos
        .settings
        .get_platform_settings()
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?;

    let next_bridge_base_url = match body.x_bridge_base_url {
        Some(ref raw) => {
            let validated = validate_optional_http_url(Some(raw), "xBridgeBaseUrl")
                .map_err(|err| ApiError::new(&req_id.0, err))?;
            if let Some(ref url) = validated {
                state
                    .ssrf
                    .validate_url(url)
                    .await
                    .map_err(|err| ApiError::new(&req_id.0, err))?;
            }
            validated
        }
        None => existing.as_ref().and_then(|r| r.x_bridge_base_url.clone()),
    };

    let next_bridge_reload_url = match body.x_bridge_reload_url {
        Some(ref raw) => {
            let validated = validate_optional_http_url(Some(raw), "xBridgeReloadUrl")
                .map_err(|err| ApiError::new(&req_id.0, err))?;
            if let Some(ref url) = validated {
                state
                    .ssrf
                    .validate_url(url)
                    .await
                    .map_err(|err| ApiError::new(&req_id.0, err))?;
            }
            validated
        }
        None => existing
            .as_ref()
            .and_then(|r| r.x_bridge_reload_url.clone()),
    };

    let normalized_auth_token = if body.clear_x_auth_token {
        None
    } else if let Some(ref raw_token) = body.x_auth_token {
        if raw_token.trim().is_empty() {
            None
        } else {
            Some(extract_x_auth_token(raw_token).map_err(|err| ApiError::new(&req_id.0, err))?)
        }
    } else {
        None
    };
    let auth_token_changed = body.clear_x_auth_token || normalized_auth_token.is_some();

    let (reload_secret_enc, reload_secret_hint) = resolve_secret_update(
        Arc::clone(&state.cipher),
        body.x_bridge_reload_secret.clone(),
        body.clear_x_bridge_reload_secret,
        existing
            .as_ref()
            .and_then(|r| r.x_bridge_reload_secret_enc.clone()),
        existing
            .as_ref()
            .and_then(|r| r.x_bridge_reload_secret_hint.clone()),
    )
    .await
    .map_err(|err| ApiError::new(&req_id.0, err))?;

    let (auth_token_enc, auth_token_hint) = resolve_secret_update(
        Arc::clone(&state.cipher),
        normalized_auth_token.clone(),
        body.clear_x_auth_token,
        existing.as_ref().and_then(|r| r.x_auth_token_enc.clone()),
        existing.as_ref().and_then(|r| r.x_auth_token_hint.clone()),
    )
    .await
    .map_err(|err| ApiError::new(&req_id.0, err))?;

    let (bearer_token_enc, bearer_token_hint) = resolve_secret_update(
        Arc::clone(&state.cipher),
        body.x_bearer_token.clone(),
        body.clear_x_bearer_token,
        existing.as_ref().and_then(|r| r.x_bearer_token_enc.clone()),
        existing
            .as_ref()
            .and_then(|r| r.x_bearer_token_hint.clone()),
    )
    .await
    .map_err(|err| ApiError::new(&req_id.0, err))?;

    let saved = state
        .repos
        .settings
        .upsert_platform_settings(UpsertPlatformSettingsRecord {
            x_bridge_base_url: next_bridge_base_url,
            x_bridge_reload_url: next_bridge_reload_url,
            x_bridge_reload_secret_enc: reload_secret_enc.clone(),
            x_bridge_reload_secret_hint: reload_secret_hint,
            x_auth_token_enc: auth_token_enc,
            x_auth_token_hint: auth_token_hint,
            x_bearer_token_enc: bearer_token_enc,
            x_bearer_token_hint: bearer_token_hint,
            updated_by: Some(user.id),
        })
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?;

    let view = resolve_platform_view(Some(&saved), &state.config);

    let (bridge_reloaded, bridge_message) = if auth_token_changed {
        if let Some(ref reload_url) = view.x_bridge_reload_url {
            let effective_secret = if let Some(enc) = saved.x_bridge_reload_secret_enc.clone() {
                let cipher = Arc::clone(&state.cipher);
                tokio::task::spawn_blocking(move || cipher.decrypt(&enc))
                    .await
                    .map_err(|err| {
                        ApiError::new(
                            &req_id.0,
                            CoreError::Internal(format!("secret decrypt task failed: {err}")),
                        )
                    })?
                    .map(Some)
                    .map_err(|err| ApiError::new(&req_id.0, err))?
            } else {
                non_empty_trimmed(state.config.platform_x_bridge_reload_secret.as_deref())
            };

            let token_payload = if body.clear_x_auth_token {
                ""
            } else {
                normalized_auth_token.as_deref().unwrap_or("")
            };

            match push_bridge_token_reload(
                &state,
                reload_url,
                token_payload,
                effective_secret.as_deref(),
            )
            .await
            {
                Ok(()) => (
                    true,
                    Some("Bridge hot-reloaded with updated X auth_token".to_owned()),
                ),
                Err(msg) => (false, Some(msg)),
            }
        } else {
            (
                false,
                Some("Saved in platform database (no bridge reload URL configured)".to_owned()),
            )
        }
    } else {
        (false, None)
    };

    Ok(Json(UpdatePlatformSettingsResponse {
        settings: view,
        bridge_reloaded,
        bridge_message,
    }))
}

/// `POST /api/v1/admin/platform/x/test` — tests platform-level X collection live.
pub async fn test_platform_x(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    OptionalApiJson(body): OptionalApiJson<PlatformXTestRequest>,
) -> Result<Json<PlatformXTestResult>, ApiError> {
    require_admin(&user, &req_id.0)?;

    let handle = non_empty_trimmed(body.handle.as_deref())
        .unwrap_or_else(|| DEFAULT_X_TEST_HANDLE.to_owned())
        .trim_start_matches('@')
        .to_owned();
    if handle.is_empty()
        || !handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(ApiError::invalid_input(
            req_id.0,
            "handle must contain only ASCII letters, digits, or underscore",
        ));
    }

    let stored = state
        .repos
        .settings
        .get_platform_settings()
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?;
    let view = resolve_platform_view(stored.as_ref(), &state.config);
    let mode = if view.x_bridge_configured {
        MODE_BRIDGE
    } else if view.has_x_bearer_token {
        MODE_OFFICIAL_API
    } else {
        MODE_UNCONFIGURED
    };

    let effective_cfg = EffectivePlatformXConfig::from_parts(
        view.x_bridge_base_url.clone(),
        stored.and_then(|s| s.x_bearer_token_enc),
    );

    let descriptor = SourceDescriptor {
        id: Uuid::new_v4(),
        source_type: SourceType::XAccount,
        scope: SourceScope::UserPrivate,
        canonical_key: format!("x_account:@{handle}"),
        name: format!("@{handle}"),
        handle_or_url: format!("@{handle}"),
        category: "Social".to_owned(),
    };

    let started = Instant::now();
    let res = state
        .executor
        .test_platform_x_descriptor(descriptor, &effective_cfg)
        .await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    match res {
        Ok(fetch_res) => {
            let sample_titles = fetch_res
                .items
                .iter()
                .take(SOURCE_TEST_SAMPLE_TITLE_LIMIT)
                .map(|i| i.title.clone())
                .collect::<Vec<_>>();
            let count = fetch_res.items.len();
            Ok(Json(PlatformXTestResult {
                ok: fetch_res.outcome != FetchOutcome::Failed,
                mode: mode.to_owned(),
                handle,
                items_fetched: count,
                sample_titles,
                latency_ms,
                message: format!("Fetched {count} items via {mode}"),
            }))
        }
        Err(err @ CoreError::SsrfBlocked(_)) => Err(ApiError::new(req_id.0, err)),
        Err(err) => Ok(Json(PlatformXTestResult {
            ok: false,
            mode: mode.to_owned(),
            handle,
            items_fetched: 0,
            sample_titles: Vec::new(),
            latency_ms,
            message: err.to_string(),
        })),
    }
}

/// Checks directly against `repos.settings` + `config` whether platform-level X collection
/// is available (used by `test_platform_x` / `import_preset_sources` / `/me`).
pub async fn compute_platform_x_ready(state: &AppState) -> Result<bool, CoreError> {
    let stored = state.repos.settings.get_platform_settings().await?;
    Ok(resolve_platform_view(stored.as_ref(), &state.config).x_integration_ready)
}

#[allow(dead_code)]
async fn _fetch_helper_link(
    state: &AppState,
    descriptor: &SourceDescriptor,
    effective: &EffectivePlatformXConfig,
) -> Result<digest_core::traits::FetchResult, CoreError> {
    let _ = fetch_with_platform_x_support;
    Err(CoreError::InvalidConfig(format!(
        "unused helper for {} ({})",
        descriptor.canonical_key,
        effective.bridge_base_url.is_some() && state.config.core.llm.max_items_per_digest > 0
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_x_auth_token_handles_raw_multi_and_cookie_header() {
        assert_eq!(
            extract_x_auth_token("  abc123def456  ").unwrap(),
            "abc123def456"
        );
        assert_eq!(
            extract_x_auth_token("tok_one, tok_two , ").unwrap(),
            "tok_one,tok_two"
        );
        let full_cookie = "cookie\nguest_id=v1%3A123; auth_token=cookie_tok_999; ct0=csrf_111;";
        assert_eq!(extract_x_auth_token(full_cookie).unwrap(), "cookie_tok_999");

        let quoted = "auth_token=\"quoted_tok_777\"; ct0=abc";
        assert_eq!(extract_x_auth_token(quoted).unwrap(), "quoted_tok_777");

        assert!(matches!(
            extract_x_auth_token("   "),
            Err(CoreError::InvalidInput(_))
        ));
        assert!(matches!(
            extract_x_auth_token("tok1\ntok2"),
            Err(CoreError::InvalidInput(_))
        ));
        assert!(matches!(
            extract_x_auth_token("auth_token=; ct0=123"),
            Err(CoreError::InvalidInput(_))
        ));
    }
}
