//! Self-service user settings endpoints: AI config, sources/presets/export/import,
//! notification channels, schedule, and keyword watchlist rules.

use std::fmt;
use std::net::IpAddr;

use axum::extract::{Path, State};
use axum::Json;
use chrono::Utc;
use digest_core::constants::{
    DEFAULT_AI_BASE_URL, DEFAULT_AI_MODEL, DEFAULT_AI_PROVIDER_LABEL, DEFAULT_LLM_MAX_TOKENS,
    DEFAULT_LLM_SYSTEM_PROMPT, DEFAULT_LLM_TEMPERATURE, HEADER_CONTENT_TYPE,
    HEADER_WEBHOOK_SIGNATURE, SECRET_MASK_REDACTED_PLACEHOLDER,
};
use digest_core::crypto::mask_secret;
use digest_core::domain::{NotificationChannelType, SourceScope, SourceType};
use digest_core::error::CoreError;
use digest_core::notifiers::dingtalk::DingTalkChannelConfig;
use digest_core::notifiers::feishu::FeishuChannelConfig;
use digest_core::notifiers::telegram::TelegramChannelConfig;
use digest_core::notifiers::webhook::WebhookChannelConfig;
use digest_core::notifiers::wecom::WeComChannelConfig;
use digest_core::notifiers::SmtpNotifier;
use digest_core::presets::{find_preset_by_key, gemini_ecosystem_presets};
use digest_core::ssrf::SsrfGuard;
use http::header::{HeaderName, HeaderValue};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::api::error::{parse_uuid, ApiError, ApiJson, OptionalApiJson, RequestId};
use crate::auth::AuthenticatedUser;
use crate::constants::SOURCE_EXPORT_FORMAT_VERSION;
use crate::repo::{
    decode_source_config, encode_source_config, CreateChannelRecord, ExportedSourceSpec,
    NotificationChannelView, SourceExportBundle, SubscribeSourceCmd, UpdateChannelRecord,
    UpdateScheduleCmd, UpdateSourceCmd, UpsertAiConfigRecord, UpsertWatchlistRuleCmd,
    UserAiConfigView, UserScheduleRecord, UserSourceView, WatchlistRuleRecord,
};
use crate::services::{AiTestRequest, AiTestResult, ChannelTestResult, SourceTestResult};
use crate::state::AppState;

const fn default_true() -> bool {
    true
}

// ============================================================================
// AI Config
// ============================================================================

/// Request payload for `PUT /api/v1/settings/ai` (redacted `Debug`).
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAiConfigRequest {
    pub provider_label: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    pub model: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub clear_api_key: bool,
}

impl fmt::Debug for UpdateAiConfigRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .api_key
            .as_ref()
            .filter(|k| !k.trim().is_empty())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("UpdateAiConfigRequest")
            .field("provider_label", &self.provider_label)
            .field("base_url", &self.base_url)
            .field("api_key", &redacted)
            .field("model", &self.model)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("clear_api_key", &self.clear_api_key)
            .finish()
    }
}

/// `GET /api/v1/settings/ai` — returns masked user AI config (or safe defaults).
pub async fn get_ai_config(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
) -> Result<Json<UserAiConfigView>, ApiError> {
    let existing = state
        .repos
        .settings
        .get_ai_config(user.id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    let view = match existing {
        Some(record) => record.to_view(),
        None => UserAiConfigView {
            provider_label: DEFAULT_AI_PROVIDER_LABEL.to_owned(),
            base_url: DEFAULT_AI_BASE_URL.to_owned(),
            api_key_hint: None,
            has_api_key: false,
            model: DEFAULT_AI_MODEL.to_owned(),
            system_prompt: DEFAULT_LLM_SYSTEM_PROMPT.to_owned(),
            temperature: DEFAULT_LLM_TEMPERATURE,
            max_tokens: DEFAULT_LLM_MAX_TOKENS,
            updated_at: None,
        },
    };

    Ok(Json(view))
}

/// `PUT /api/v1/settings/ai` — validates SSRF, encrypts API key, and returns masked view.
pub async fn update_ai_config(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiJson(body): ApiJson<UpdateAiConfigRequest>,
) -> Result<Json<UserAiConfigView>, ApiError> {
    let provider_label = body.provider_label.trim();
    if provider_label.is_empty() {
        return Err(ApiError::invalid_input(
            req_id.0,
            "providerLabel must not be empty",
        ));
    }

    let model = body.model.trim();
    if model.is_empty() {
        return Err(ApiError::invalid_input(req_id.0, "model must not be empty"));
    }

    let base_url = body.base_url.trim();
    let _ = state
        .ssrf
        .validate_url(base_url)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?;

    let temperature = body.temperature.unwrap_or(DEFAULT_LLM_TEMPERATURE);
    if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
        return Err(ApiError::invalid_input(
            req_id.0,
            "temperature must be between 0.0 and 2.0",
        ));
    }

    let max_tokens_u32 = body.max_tokens.unwrap_or(DEFAULT_LLM_MAX_TOKENS);
    if max_tokens_u32 == 0 {
        return Err(ApiError::invalid_input(req_id.0, "maxTokens must be >= 1"));
    }
    let max_tokens = i32::try_from(max_tokens_u32)
        .map_err(|_| ApiError::invalid_input(&req_id.0, "maxTokens exceeds maximum"))?;

    let system_prompt = body
        .system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_LLM_SYSTEM_PROMPT)
        .to_owned();

    let existing = state
        .repos
        .settings
        .get_ai_config(user.id)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?;

    let (api_key_enc, api_key_hint) = if body.clear_api_key {
        (None, None)
    } else if let Some(raw_key) = body
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        let hint = mask_secret(raw_key);
        let cipher = state.cipher.clone();
        let plaintext = raw_key.to_owned();
        let enc = tokio::task::spawn_blocking(move || cipher.encrypt(&plaintext))
            .await
            .map_err(|err| {
                ApiError::new(
                    &req_id.0,
                    CoreError::Internal(format!("encryption task failed: {err}")),
                )
            })?
            .map_err(|err| ApiError::new(&req_id.0, err))?;
        (Some(enc), Some(hint))
    } else if let Some(prev) = existing {
        (prev.api_key_enc, prev.api_key_hint)
    } else {
        (None, None)
    };

    let cmd = UpsertAiConfigRecord {
        provider_label: provider_label.to_owned(),
        base_url: base_url.to_owned(),
        api_key_enc,
        api_key_hint,
        model: model.to_owned(),
        system_prompt,
        temperature,
        max_tokens,
    };

    let saved = state
        .repos
        .settings
        .upsert_ai_config(user.id, cmd)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(saved.to_view()))
}

/// `POST /api/v1/settings/ai/test` — SSRF-checks `baseUrl` (when supplied) and tests AI connectivity.
pub async fn test_ai_config(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    OptionalApiJson(body): OptionalApiJson<AiTestRequest>,
) -> Result<Json<AiTestResult>, ApiError> {
    if let Some(raw_url) = body
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        let _ = state
            .ssrf
            .validate_url(raw_url)
            .await
            .map_err(|err| ApiError::new(&req_id.0, err))?;
    }

    let result = state
        .executor
        .test_ai_config(user.id, body)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(result))
}

// ============================================================================
// Sources, Presets, Export / Import & Live Preview
// ============================================================================

/// Request payload for `POST /api/v1/settings/sources` (redacted `Debug`).
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSourceRequest {
    pub source_type: SourceType,
    #[serde(default)]
    pub scope: Option<SourceScope>,
    #[serde(default)]
    pub canonical_key: Option<String>,
    #[serde(default)]
    pub custom_name: Option<String>,
    #[serde(default)]
    pub default_name: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    pub handle_or_url: String,
    #[serde(default)]
    pub credentials: Option<serde_json::Value>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl fmt::Debug for CreateSourceRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .credentials
            .as_ref()
            .filter(|v| !v.is_null())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("CreateSourceRequest")
            .field("source_type", &self.source_type)
            .field("scope", &self.scope)
            .field("canonical_key", &self.canonical_key)
            .field("custom_name", &self.custom_name)
            .field("category", &self.category)
            .field("handle_or_url", &self.handle_or_url)
            .field("credentials", &redacted)
            .field("enabled", &self.enabled)
            .finish()
    }
}

/// Request payload for `PUT /api/v1/settings/sources/{id}` (redacted `Debug`).
#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSourceRequest {
    #[serde(default)]
    pub custom_name: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub handle_or_url: Option<String>,
    #[serde(default)]
    pub credentials: Option<serde_json::Value>,
    #[serde(default)]
    pub clear_credentials: bool,
}

impl fmt::Debug for UpdateSourceRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .credentials
            .as_ref()
            .filter(|v| !v.is_null())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("UpdateSourceRequest")
            .field("custom_name", &self.custom_name)
            .field("category", &self.category)
            .field("enabled", &self.enabled)
            .field("handle_or_url", &self.handle_or_url)
            .field("credentials", &redacted)
            .field("clear_credentials", &self.clear_credentials)
            .finish()
    }
}

/// Optional request payload for `POST /api/v1/settings/sources/import-preset`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPresetRequest {
    #[serde(default)]
    pub canonical_keys: Option<Vec<String>>,
    #[serde(default)]
    pub include_x_templates: bool,
}

/// Response returned by `import-preset` and `import`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSourcesResponse {
    pub imported_count: usize,
    pub sources: Vec<UserSourceView>,
}

async fn validate_source_handle_or_url(
    ssrf: &SsrfGuard,
    source_type: SourceType,
    handle_or_url: &str,
) -> Result<String, CoreError> {
    let trimmed = handle_or_url.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(
            "handleOrUrl must not be empty".to_owned(),
        ));
    }
    if source_type == SourceType::Rss || trimmed.contains("://") {
        let _ = ssrf.validate_url(trimmed).await?;
    }
    Ok(trimmed.to_owned())
}

fn has_non_empty_credentials(credentials: Option<&serde_json::Value>) -> bool {
    match credentials {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Object(map)) => !map.is_empty(),
        Some(serde_json::Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
    }
}

/// `GET /api/v1/settings/sources` — lists user's subscribed sources with masked credential hints.
pub async fn list_sources(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
) -> Result<Json<Vec<UserSourceView>>, ApiError> {
    let sources = state
        .repos
        .sources
        .list_user_sources(user.id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(sources))
}

/// `POST /api/v1/settings/sources` — validates SSRF, encrypts config, and subscribes/creates source.
pub async fn create_source(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiJson(body): ApiJson<CreateSourceRequest>,
) -> Result<Json<UserSourceView>, ApiError> {
    let handle_or_url =
        validate_source_handle_or_url(&state.ssrf, body.source_type, &body.handle_or_url)
            .await
            .map_err(|err| ApiError::new(&req_id.0, err))?;

    let has_creds = has_non_empty_credentials(body.credentials.as_ref());
    let requires_private =
        has_creds || matches!(body.source_type, SourceType::XAccount | SourceType::XSearch);

    let scope = match body.scope {
        Some(SourceScope::SharedPublic) if requires_private => {
            return Err(ApiError::invalid_input(
                req_id.0,
                "sources with credentials or X API templates must use user_private scope",
            ));
        }
        Some(explicit) => explicit,
        None if requires_private => SourceScope::UserPrivate,
        None => SourceScope::SharedPublic,
    };

    let canonical_key = body
        .canonical_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{}:{}",
                body.source_type.as_str(),
                handle_or_url.to_ascii_lowercase()
            )
        });

    let default_name = body
        .default_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            body.custom_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(&handle_or_url)
        .to_owned();

    let custom_name = body
        .custom_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&default_name)
        .to_owned();

    let category = body
        .category
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Custom")
        .to_owned();

    let credentials = if has_creds { body.credentials } else { None };
    let cipher = state.cipher.clone();
    let handle_for_enc = handle_or_url.clone();
    let config_json_enc = tokio::task::spawn_blocking(move || {
        encode_source_config(&cipher, &handle_for_enc, credentials)
    })
    .await
    .map_err(|err| {
        ApiError::new(
            &req_id.0,
            CoreError::Internal(format!("encryption task failed: {err}")),
        )
    })?
    .map_err(|err| ApiError::new(&req_id.0, err))?;

    let cmd = SubscribeSourceCmd {
        source_type: body.source_type,
        scope,
        canonical_key,
        custom_name,
        default_name,
        category,
        handle_or_url,
        config_json_enc: Some(config_json_enc),
        enabled: body.enabled,
    };

    let created = state
        .repos
        .sources
        .create_or_subscribe(user.id, cmd)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(created))
}

/// `PUT /api/v1/settings/sources/{id}` — updates subscription metadata and private source config.
pub async fn update_source(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateSourceRequest>,
) -> Result<Json<UserSourceView>, ApiError> {
    let source_or_sub_id =
        parse_uuid(&id, "sourceId").map_err(|err| ApiError::new(&req_id.0, err))?;

    let existing = state
        .repos
        .sources
        .get_user_source(user.id, source_or_sub_id)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?
        .ok_or_else(|| {
            ApiError::new(
                &req_id.0,
                CoreError::NotFound(format!("subscribed source {source_or_sub_id} not found")),
            )
        })?;

    let custom_name = match body.custom_name {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(ApiError::invalid_input(
                    req_id.0,
                    "customName must not be empty",
                ));
            }
            Some(trimmed.to_owned())
        }
        None => None,
    };

    let category = match body.category {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(ApiError::invalid_input(
                    req_id.0,
                    "category must not be empty",
                ));
            }
            Some(trimmed.to_owned())
        }
        None => None,
    };

    let new_handle_or_url = match body.handle_or_url {
        Some(ref raw) => Some(
            validate_source_handle_or_url(&state.ssrf, existing.source_type, raw)
                .await
                .map_err(|err| ApiError::new(&req_id.0, err))?,
        ),
        None => None,
    };

    let has_new_creds = has_non_empty_credentials(body.credentials.as_ref());

    let (final_handle, final_enc) = if existing.scope == SourceScope::SharedPublic {
        if has_new_creds || body.clear_credentials {
            return Err(ApiError::invalid_input(
                req_id.0,
                "cannot attach credentials to a shared_public source",
            ));
        }
        if let Some(ref requested_handle) = new_handle_or_url {
            let decoded_existing = decode_source_config(
                &state.cipher,
                &existing.canonical_key,
                existing.config_json_enc.as_deref(),
            )
            .map_err(|err| ApiError::new(&req_id.0, err))?;
            if requested_handle != &decoded_existing.handle_or_url {
                return Err(ApiError::invalid_input(
                    req_id.0,
                    "cannot modify handleOrUrl on a shared_public source",
                ));
            }
        }
        (None, None)
    } else if new_handle_or_url.is_some() || has_new_creds || body.clear_credentials {
        let cipher = state.cipher.clone();
        let canonical_key = existing.canonical_key.clone();
        let existing_enc = existing.config_json_enc.clone();
        let requested_handle = new_handle_or_url.clone();
        let requested_creds = body.credentials.clone();
        let clear_creds = body.clear_credentials;

        let (resolved_handle, reencrypted) = tokio::task::spawn_blocking(move || {
            let prev = decode_source_config(&cipher, &canonical_key, existing_enc.as_deref())?;
            let next_handle = requested_handle.unwrap_or(prev.handle_or_url);
            let next_creds = if clear_creds {
                None
            } else if has_new_creds {
                requested_creds
            } else {
                prev.credentials
            };
            let enc = encode_source_config(&cipher, &next_handle, next_creds)?;
            Ok::<_, CoreError>((next_handle, enc))
        })
        .await
        .map_err(|err| {
            ApiError::new(
                &req_id.0,
                CoreError::Internal(format!("encryption task failed: {err}")),
            )
        })?
        .map_err(|err| ApiError::new(&req_id.0, err))?;

        (Some(resolved_handle), Some(reencrypted))
    } else {
        (None, None)
    };

    let cmd = UpdateSourceCmd {
        custom_name,
        category,
        enabled: body.enabled,
        handle_or_url: final_handle,
        config_json_enc: final_enc,
    };

    let updated = state
        .repos
        .sources
        .update_user_subscription(user.id, source_or_sub_id, cmd)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(updated))
}

/// `DELETE /api/v1/settings/sources/{id}` — unsubscribes user (and cleans up private sources).
pub async fn delete_source(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let source_or_sub_id =
        parse_uuid(&id, "sourceId").map_err(|err| ApiError::new(&req_id.0, err))?;

    state
        .repos
        .sources
        .delete_user_subscription(user.id, source_or_sub_id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/settings/sources/import-preset` — subscribes user to Gemini Ecosystem Preset Pack.
pub async fn import_preset_sources(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    OptionalApiJson(body): OptionalApiJson<ImportPresetRequest>,
) -> Result<Json<ImportSourcesResponse>, ApiError> {
    let presets = match body.canonical_keys {
        Some(keys) if !keys.is_empty() => {
            let mut selected = Vec::with_capacity(keys.len());
            for key in keys {
                let preset = find_preset_by_key(&key).ok_or_else(|| {
                    ApiError::invalid_input(&req_id.0, format!("unknown preset key '{key}'"))
                })?;
                selected.push(preset);
            }
            selected
        }
        _ => gemini_ecosystem_presets()
            .into_iter()
            .filter(|preset| body.include_x_templates || preset.scope == SourceScope::SharedPublic)
            .collect(),
    };

    let mut imported = Vec::with_capacity(presets.len());
    for preset in presets {
        let cmd = SubscribeSourceCmd {
            source_type: preset.source_type,
            scope: preset.scope,
            canonical_key: preset.canonical_key,
            custom_name: preset.default_name.clone(),
            default_name: preset.default_name,
            category: preset.category,
            handle_or_url: preset.handle_or_url,
            config_json_enc: None,
            enabled: true,
        };
        let view = state
            .repos
            .sources
            .create_or_subscribe(user.id, cmd)
            .await
            .map_err(|err| ApiError::new(&req_id.0, err))?;
        imported.push(view);
    }

    Ok(Json(ImportSourcesResponse {
        imported_count: imported.len(),
        sources: imported,
    }))
}

/// `POST /api/v1/settings/sources/export` — exports non-secret source definitions JSON.
pub async fn export_sources(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
) -> Result<Json<SourceExportBundle>, ApiError> {
    let sources = state
        .repos
        .sources
        .list_user_sources(user.id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    let exported = sources
        .into_iter()
        .map(|src| ExportedSourceSpec {
            source_type: src.source_type,
            scope: src.scope,
            canonical_key: src.canonical_key,
            custom_name: src.custom_name,
            category: src.category,
            handle_or_url: src.handle_or_url,
            enabled: src.enabled,
        })
        .collect();

    Ok(Json(SourceExportBundle {
        version: SOURCE_EXPORT_FORMAT_VERSION,
        exported_at: Utc::now(),
        sources: exported,
    }))
}

/// `POST /api/v1/settings/sources/import` — imports portable non-secret source definitions JSON.
pub async fn import_sources(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiJson(bundle): ApiJson<SourceExportBundle>,
) -> Result<Json<ImportSourcesResponse>, ApiError> {
    if bundle.version != SOURCE_EXPORT_FORMAT_VERSION {
        return Err(ApiError::invalid_input(
            req_id.0,
            format!(
                "unsupported source export bundle version {}; expected {SOURCE_EXPORT_FORMAT_VERSION}",
                bundle.version
            ),
        ));
    }

    let mut prepared = Vec::with_capacity(bundle.sources.len());
    for spec in bundle.sources {
        let canonical_key = spec.canonical_key.trim();
        let custom_name = spec.custom_name.trim();
        let category = spec.category.trim();
        if canonical_key.is_empty() || custom_name.is_empty() || category.is_empty() {
            return Err(ApiError::invalid_input(
                req_id.0,
                "imported sources must include non-empty canonicalKey, customName, and category",
            ));
        }

        let handle_or_url =
            validate_source_handle_or_url(&state.ssrf, spec.source_type, &spec.handle_or_url)
                .await
                .map_err(|err| ApiError::new(&req_id.0, err))?;

        let cipher = state.cipher.clone();
        let handle_for_enc = handle_or_url.clone();
        let enc = tokio::task::spawn_blocking(move || {
            encode_source_config(&cipher, &handle_for_enc, None)
        })
        .await
        .map_err(|err| {
            ApiError::new(
                &req_id.0,
                CoreError::Internal(format!("encryption task failed: {err}")),
            )
        })?
        .map_err(|err| ApiError::new(&req_id.0, err))?;

        prepared.push(SubscribeSourceCmd {
            source_type: spec.source_type,
            scope: spec.scope,
            canonical_key: canonical_key.to_owned(),
            custom_name: custom_name.to_owned(),
            default_name: custom_name.to_owned(),
            category: category.to_owned(),
            handle_or_url,
            config_json_enc: Some(enc),
            enabled: spec.enabled,
        });
    }

    let mut imported = Vec::with_capacity(prepared.len());
    for cmd in prepared {
        let view = state
            .repos
            .sources
            .create_or_subscribe(user.id, cmd)
            .await
            .map_err(|err| ApiError::new(&req_id.0, err))?;
        imported.push(view);
    }

    Ok(Json(ImportSourcesResponse {
        imported_count: imported.len(),
        sources: imported,
    }))
}

/// `POST /api/v1/settings/sources/{id}/test` — live-fetches a single subscribed source.
pub async fn test_source(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(id): Path<String>,
) -> Result<Json<SourceTestResult>, ApiError> {
    let source_or_sub_id =
        parse_uuid(&id, "sourceId").map_err(|err| ApiError::new(&req_id.0, err))?;

    let source = state
        .repos
        .sources
        .get_user_source(user.id, source_or_sub_id)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?
        .ok_or_else(|| {
            ApiError::new(
                &req_id.0,
                CoreError::NotFound(format!("subscribed source {source_or_sub_id} not found")),
            )
        })?;

    let result = state
        .executor
        .test_source(user.id, source.canonical_source_id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(result))
}

// ============================================================================
// Notification Channels
// ============================================================================

/// Request payload for `POST /api/v1/settings/channels` (redacted `Debug`).
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateChannelRequest {
    pub channel_type: NotificationChannelType,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub config: serde_json::Value,
    #[serde(default = "default_true")]
    pub trigger_on_daily: bool,
    #[serde(default = "default_true")]
    pub trigger_on_alert: bool,
}

impl fmt::Debug for CreateChannelRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateChannelRequest")
            .field("channel_type", &self.channel_type)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("config", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .field("trigger_on_daily", &self.trigger_on_daily)
            .field("trigger_on_alert", &self.trigger_on_alert)
            .finish()
    }
}

/// Request payload for `PUT /api/v1/settings/channels/{id}` (redacted `Debug`).
#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateChannelRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub trigger_on_daily: Option<bool>,
    #[serde(default)]
    pub trigger_on_alert: Option<bool>,
}

impl fmt::Debug for UpdateChannelRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .config
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("UpdateChannelRequest")
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("config", &redacted)
            .field("trigger_on_daily", &self.trigger_on_daily)
            .field("trigger_on_alert", &self.trigger_on_alert)
            .finish()
    }
}

fn mask_webhook_url_hint(raw_url: &str) -> String {
    if let Ok(parsed) = Url::parse(raw_url.trim()) {
        if let Some(host) = parsed.host_str() {
            return format!(
                "{}://{host}/{SECRET_MASK_REDACTED_PLACEHOLDER}",
                parsed.scheme()
            );
        }
    }
    mask_secret(raw_url)
}

async fn validate_and_mask_channel_config(
    ssrf: &SsrfGuard,
    channel_type: NotificationChannelType,
    config: &serde_json::Value,
) -> Result<String, CoreError> {
    match channel_type {
        NotificationChannelType::Smtp => {
            let cfg = SmtpNotifier::parse_config(config)?;
            let raw_host = cfg.host.trim();
            let unbracketed = raw_host.trim_start_matches('[').trim_end_matches(']');
            if let Ok(ip) = unbracketed.parse::<IpAddr>() {
                if !ssrf.is_allowlisted(unbracketed) {
                    ssrf.validate_ip(ip)?;
                }
                let probe_host = match ip {
                    IpAddr::V4(v4) => v4.to_string(),
                    IpAddr::V6(v6) => format!("[{v6}]"),
                };
                let probe_url = format!("https://{probe_host}:{}/", cfg.port);
                let _ = ssrf.validate_url(&probe_url).await?;
            } else {
                let probe_url = format!("https://{raw_host}:{}/", cfg.port);
                let _ = ssrf.validate_url(&probe_url).await?;
            }
            Ok(format!(
                "{}:{} -> {}",
                cfg.host.trim(),
                cfg.port,
                cfg.to.join(", ")
            ))
        }
        NotificationChannelType::Telegram => {
            let cfg: TelegramChannelConfig =
                serde_json::from_value(config.clone()).map_err(|err| {
                    CoreError::InvalidConfig(format!("invalid Telegram config: {err}"))
                })?;
            let bot_token = cfg.bot_token.trim();
            let chat_id = cfg.chat_id.trim();
            if bot_token.is_empty() || chat_id.is_empty() {
                return Err(CoreError::InvalidConfig(
                    "Telegram botToken and chatId are required".to_owned(),
                ));
            }
            if let Some(base) = cfg
                .api_base_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let _ = ssrf.validate_url(base).await?;
            }
            Ok(format!("chat:{chat_id} token:{}", mask_secret(bot_token)))
        }
        NotificationChannelType::Feishu => {
            let cfg: FeishuChannelConfig = serde_json::from_value(config.clone())
                .map_err(|err| CoreError::InvalidConfig(format!("invalid Feishu config: {err}")))?;
            let _ = ssrf.validate_url(cfg.webhook_url.trim()).await?;
            Ok(mask_webhook_url_hint(&cfg.webhook_url))
        }
        NotificationChannelType::Dingtalk => {
            let cfg: DingTalkChannelConfig =
                serde_json::from_value(config.clone()).map_err(|err| {
                    CoreError::InvalidConfig(format!("invalid DingTalk config: {err}"))
                })?;
            let _ = ssrf.validate_url(cfg.webhook_url.trim()).await?;
            Ok(mask_webhook_url_hint(&cfg.webhook_url))
        }
        NotificationChannelType::Wecom => {
            let cfg: WeComChannelConfig = serde_json::from_value(config.clone())
                .map_err(|err| CoreError::InvalidConfig(format!("invalid WeCom config: {err}")))?;
            let _ = ssrf.validate_url(cfg.webhook_url.trim()).await?;
            Ok(mask_webhook_url_hint(&cfg.webhook_url))
        }
        NotificationChannelType::Webhook => {
            let cfg: WebhookChannelConfig =
                serde_json::from_value(config.clone()).map_err(|err| {
                    CoreError::InvalidConfig(format!("invalid webhook config: {err}"))
                })?;
            let _ = ssrf.validate_url(cfg.url.trim()).await?;
            for (raw_name, raw_value) in &cfg.headers {
                let header_name =
                    HeaderName::from_bytes(raw_name.trim().as_bytes()).map_err(|err| {
                        CoreError::InvalidInput(format!(
                            "invalid webhook header name '{raw_name}': {err}"
                        ))
                    })?;
                let lower = header_name.as_str();
                if lower == HEADER_CONTENT_TYPE || lower == HEADER_WEBHOOK_SIGNATURE {
                    return Err(CoreError::InvalidInput(format!(
                        "webhook header '{lower}' is managed automatically"
                    )));
                }
                let _ = HeaderValue::from_str(raw_value.trim()).map_err(|err| {
                    CoreError::InvalidInput(format!(
                        "invalid webhook header value for '{lower}': {err}"
                    ))
                })?;
            }
            Ok(mask_webhook_url_hint(&cfg.url))
        }
    }
}

/// `GET /api/v1/settings/channels` — lists user's notification channels (masked hints only).
pub async fn list_channels(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
) -> Result<Json<Vec<NotificationChannelView>>, ApiError> {
    let records = state
        .repos
        .settings
        .list_channels(user.id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    let views = records.into_iter().map(|r| r.to_view()).collect();
    Ok(Json(views))
}

/// `POST /api/v1/settings/channels` — validates channel config & SSRF, encrypts, and stores it.
pub async fn create_channel(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiJson(body): ApiJson<CreateChannelRequest>,
) -> Result<Json<NotificationChannelView>, ApiError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::invalid_input(
            req_id.0,
            "channel name must not be empty",
        ));
    }

    let config_hint =
        validate_and_mask_channel_config(&state.ssrf, body.channel_type, &body.config)
            .await
            .map_err(|err| ApiError::new(&req_id.0, err))?;

    let config_json_str = serde_json::to_string(&body.config).map_err(|err| {
        ApiError::new(
            &req_id.0,
            CoreError::Internal(format!("failed to serialize channel config: {err}")),
        )
    })?;

    let cipher = state.cipher.clone();
    let config_enc = tokio::task::spawn_blocking(move || cipher.encrypt(&config_json_str))
        .await
        .map_err(|err| {
            ApiError::new(
                &req_id.0,
                CoreError::Internal(format!("encryption task failed: {err}")),
            )
        })?
        .map_err(|err| ApiError::new(&req_id.0, err))?;

    let created = state
        .repos
        .settings
        .create_channel(
            user.id,
            CreateChannelRecord {
                channel_type: body.channel_type,
                name: name.to_owned(),
                enabled: body.enabled,
                config_enc,
                config_hint,
                trigger_on_daily: body.trigger_on_daily,
                trigger_on_alert: body.trigger_on_alert,
            },
        )
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(created.to_view()))
}

/// `PUT /api/v1/settings/channels/{id}` — updates channel config/flags and returns masked view.
pub async fn update_channel(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateChannelRequest>,
) -> Result<Json<NotificationChannelView>, ApiError> {
    let channel_id = parse_uuid(&id, "channelId").map_err(|err| ApiError::new(&req_id.0, err))?;

    let existing = state
        .repos
        .settings
        .get_channel(user.id, channel_id)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?
        .ok_or_else(|| {
            ApiError::new(
                &req_id.0,
                CoreError::NotFound(format!("notification channel {channel_id} not found")),
            )
        })?;

    let name = match body.name {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(ApiError::invalid_input(
                    req_id.0,
                    "channel name must not be empty",
                ));
            }
            Some(trimmed.to_owned())
        }
        None => None,
    };

    let (config_enc, config_hint) = if let Some(ref cfg_val) = body.config {
        let hint = validate_and_mask_channel_config(&state.ssrf, existing.channel_type, cfg_val)
            .await
            .map_err(|err| ApiError::new(&req_id.0, err))?;
        let raw_json = serde_json::to_string(cfg_val).map_err(|err| {
            ApiError::new(
                &req_id.0,
                CoreError::Internal(format!("failed to serialize channel config: {err}")),
            )
        })?;
        let cipher = state.cipher.clone();
        let enc = tokio::task::spawn_blocking(move || cipher.encrypt(&raw_json))
            .await
            .map_err(|err| {
                ApiError::new(
                    &req_id.0,
                    CoreError::Internal(format!("encryption task failed: {err}")),
                )
            })?
            .map_err(|err| ApiError::new(&req_id.0, err))?;
        (Some(enc), Some(hint))
    } else {
        (None, None)
    };

    let updated = state
        .repos
        .settings
        .update_channel(
            user.id,
            channel_id,
            UpdateChannelRecord {
                name,
                enabled: body.enabled,
                config_enc,
                config_hint,
                trigger_on_daily: body.trigger_on_daily,
                trigger_on_alert: body.trigger_on_alert,
            },
        )
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(updated.to_view()))
}

/// `DELETE /api/v1/settings/channels/{id}` — deletes a user notification channel.
pub async fn delete_channel(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let channel_id = parse_uuid(&id, "channelId").map_err(|err| ApiError::new(&req_id.0, err))?;

    state
        .repos
        .settings
        .delete_channel(user.id, channel_id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/settings/channels/{id}/test` — sends a test notification via the channel.
pub async fn test_channel(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(id): Path<String>,
) -> Result<Json<ChannelTestResult>, ApiError> {
    let channel_id = parse_uuid(&id, "channelId").map_err(|err| ApiError::new(&req_id.0, err))?;

    let result = state
        .executor
        .test_channel(user.id, channel_id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(result))
}

// ============================================================================
// Schedule & Watchlist
// ============================================================================

/// `GET /api/v1/settings/schedule` — reads user schedule and email master switch.
pub async fn get_schedule(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
) -> Result<Json<UserScheduleRecord>, ApiError> {
    let schedule = state
        .repos
        .settings
        .get_schedule(user.id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(schedule))
}

/// `PUT /api/v1/settings/schedule` — validates timezone/time/interval and updates schedule.
pub async fn update_schedule(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiJson(mut body): ApiJson<UpdateScheduleCmd>,
) -> Result<Json<UserScheduleRecord>, ApiError> {
    body.timezone = body.timezone.trim().to_owned();
    body.daily_time = body.daily_time.trim().to_owned();
    body.validate()
        .map_err(|err| ApiError::new(&req_id.0, err))?;

    let updated = state
        .repos
        .settings
        .upsert_schedule(user.id, body)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(updated))
}

fn normalize_watchlist_cmd(
    mut cmd: UpsertWatchlistRuleCmd,
) -> Result<UpsertWatchlistRuleCmd, CoreError> {
    cmd.name = cmd.name.trim().to_owned();
    if cmd.name.is_empty() {
        return Err(CoreError::InvalidInput(
            "watchlist rule name must not be empty".to_owned(),
        ));
    }
    cmd.keywords = cmd
        .keywords
        .into_iter()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
        .collect();
    if cmd.keywords.is_empty() {
        return Err(CoreError::InvalidInput(
            "watchlist rule requires at least one keyword".to_owned(),
        ));
    }
    cmd.exclude_keywords = cmd
        .exclude_keywords
        .into_iter()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
        .collect();
    Ok(cmd)
}

/// `GET /api/v1/settings/watchlist` — lists user's keyword watchlist rules.
pub async fn list_watchlist(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
) -> Result<Json<Vec<WatchlistRuleRecord>>, ApiError> {
    let rules = state
        .repos
        .settings
        .list_watchlist_rules(user.id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(rules))
}

/// `POST /api/v1/settings/watchlist` — creates a keyword watchlist rule.
pub async fn create_watchlist(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiJson(body): ApiJson<UpsertWatchlistRuleCmd>,
) -> Result<Json<WatchlistRuleRecord>, ApiError> {
    let cmd = normalize_watchlist_cmd(body).map_err(|err| ApiError::new(&req_id.0, err))?;

    let created = state
        .repos
        .settings
        .create_watchlist_rule(user.id, cmd)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(created))
}

/// `PUT /api/v1/settings/watchlist/{id}` — updates an existing watchlist rule.
pub async fn update_watchlist(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpsertWatchlistRuleCmd>,
) -> Result<Json<WatchlistRuleRecord>, ApiError> {
    let rule_id = parse_uuid(&id, "ruleId").map_err(|err| ApiError::new(&req_id.0, err))?;
    let cmd = normalize_watchlist_cmd(body).map_err(|err| ApiError::new(&req_id.0, err))?;

    let updated = state
        .repos
        .settings
        .update_watchlist_rule(user.id, rule_id, cmd)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(updated))
}

/// `DELETE /api/v1/settings/watchlist/{id}` — deletes a watchlist rule.
pub async fn delete_watchlist(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let rule_id = parse_uuid(&id, "ruleId").map_err(|err| ApiError::new(&req_id.0, err))?;

    state
        .repos
        .settings
        .delete_watchlist_rule(user.id, rule_id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(StatusCode::NO_CONTENT)
}
