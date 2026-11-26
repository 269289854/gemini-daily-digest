//! Shared domain, repository, and API DTOs (`camelCase` JSON payloads,
//! secret-redacted `Debug` implementations, and pagination helpers).

use std::fmt;

use chrono::{DateTime, NaiveDate, Utc};
use digest_core::constants::{
    DEFAULT_MAX_PAGE_SIZE, DEFAULT_PAGE_SIZE, SECRET_MASK_CONFIGURED_PLACEHOLDER,
    SECRET_MASK_REDACTED_PLACEHOLDER,
};
use digest_core::crypto::{mask_secret, EnvelopeCipher};
use digest_core::domain::{
    AlertPriority, DeliveryStatus, DigestSection, FetchOutcome, NotificationChannelType, RunStatus,
    RunType, SourceScope, SourceType, WatchlistRuleInput,
};
use digest_core::error::CoreError;
use digest_core::presets::find_preset_by_key;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ============================================================================
// Pagination
// ============================================================================

/// Validated pagination parameters (`page >= 1`, `1 <= page_size <= DEFAULT_MAX_PAGE_SIZE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRequest {
    pub page: usize,
    pub page_size: usize,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: DEFAULT_PAGE_SIZE,
        }
    }
}

impl PageRequest {
    /// Builds a validated `PageRequest`, rejecting zero page/pageSize and clamping to max.
    pub fn new(page: Option<usize>, page_size: Option<usize>) -> Result<Self, CoreError> {
        let page = page.unwrap_or(1);
        if page == 0 {
            return Err(CoreError::InvalidInput("page must be >= 1".to_owned()));
        }
        let raw_size = page_size.unwrap_or(DEFAULT_PAGE_SIZE);
        if raw_size == 0 {
            return Err(CoreError::InvalidInput("pageSize must be >= 1".to_owned()));
        }
        let page_size = raw_size.min(DEFAULT_MAX_PAGE_SIZE);
        Ok(Self { page, page_size })
    }

    /// Zero-based SQL offset.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.page.saturating_sub(1).saturating_mul(self.page_size)
    }

    /// SQL row limit.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.page_size
    }
}

/// Standard paginated JSON envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PagedResult<T> {
    pub items: Vec<T>,
    pub page: usize,
    pub page_size: usize,
    pub total: u64,
    pub total_pages: u64,
}

impl<T> PagedResult<T> {
    /// Wraps page items and total item count into a `PagedResult`.
    #[must_use]
    pub fn new(items: Vec<T>, page: PageRequest, total: u64) -> Self {
        let size_u64 = page.page_size.max(1) as u64;
        let total_pages = if total == 0 {
            0
        } else {
            total.div_ceil(size_u64)
        };
        Self {
            items,
            page: page.page,
            page_size: page.page_size,
            total,
            total_pages,
        }
    }
}

// ============================================================================
// Users & Onboarding
// ============================================================================

/// Persisted user identity record (`users` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRecord {
    pub id: Uuid,
    pub oidc_sub: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

/// Command to upsert an authenticated OIDC or local dev user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertUserCmd {
    pub oidc_sub: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub role: String,
}

/// Onboarding completion flags returned by `GET /api/v1/me`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingFlags {
    pub has_ai_config: bool,
    pub has_sources: bool,
    pub has_channels: bool,
    pub has_schedule: bool,
}

// ============================================================================
// User AI Config
// ============================================================================

/// Internal persisted AI config record (`user_ai_configs` table, redacted `Debug`).
#[derive(Clone, PartialEq)]
pub struct UserAiConfigRecord {
    pub user_id: Uuid,
    pub provider_label: String,
    pub base_url: String,
    pub api_key_enc: Option<String>,
    pub api_key_hint: Option<String>,
    pub model: String,
    pub system_prompt: String,
    pub temperature: f32,
    pub max_tokens: i32,
    pub updated_at: DateTime<Utc>,
}

impl fmt::Debug for UserAiConfigRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .api_key_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("UserAiConfigRecord")
            .field("user_id", &self.user_id)
            .field("provider_label", &self.provider_label)
            .field("base_url", &self.base_url)
            .field("api_key_enc", &redacted)
            .field("api_key_hint", &self.api_key_hint)
            .field("model", &self.model)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

impl UserAiConfigRecord {
    /// Converts this record into a secret-free API response view.
    #[must_use]
    pub fn to_view(&self) -> UserAiConfigView {
        UserAiConfigView {
            provider_label: self.provider_label.clone(),
            base_url: self.base_url.clone(),
            api_key_hint: self.api_key_hint.clone(),
            has_api_key: self
                .api_key_enc
                .as_deref()
                .is_some_and(|enc| !enc.trim().is_empty()),
            model: self.model.clone(),
            system_prompt: self.system_prompt.clone(),
            temperature: self.temperature,
            max_tokens: u32::try_from(self.max_tokens.max(1)).unwrap_or(1),
            updated_at: Some(self.updated_at),
        }
    }
}

/// Command to upsert encrypted user AI configuration.
#[derive(Clone, PartialEq)]
pub struct UpsertAiConfigRecord {
    pub provider_label: String,
    pub base_url: String,
    pub api_key_enc: Option<String>,
    pub api_key_hint: Option<String>,
    pub model: String,
    pub system_prompt: String,
    pub temperature: f32,
    pub max_tokens: i32,
}

impl fmt::Debug for UpsertAiConfigRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .api_key_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("UpsertAiConfigRecord")
            .field("provider_label", &self.provider_label)
            .field("base_url", &self.base_url)
            .field("api_key_enc", &redacted)
            .field("api_key_hint", &self.api_key_hint)
            .field("model", &self.model)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .finish()
    }
}

/// Non-secret user AI configuration returned by `GET`/`PUT /api/v1/settings/ai`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserAiConfigView {
    pub provider_label: String,
    pub base_url: String,
    pub api_key_hint: Option<String>,
    pub has_api_key: bool,
    pub model: String,
    pub system_prompt: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub updated_at: Option<DateTime<Utc>>,
}

// ============================================================================
// Platform Settings (Admin-Managed Global Integrations)
// ============================================================================

/// Persisted singleton `platform_settings` row (encrypted secrets, redacted `Debug`).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct PlatformSettingsRecord {
    pub x_bridge_base_url: Option<String>,
    pub x_bridge_reload_url: Option<String>,
    pub x_bridge_reload_secret_enc: Option<String>,
    pub x_bridge_reload_secret_hint: Option<String>,
    pub x_auth_token_enc: Option<String>,
    pub x_auth_token_hint: Option<String>,
    pub x_bearer_token_enc: Option<String>,
    pub x_bearer_token_hint: Option<String>,
    pub updated_by: Option<Uuid>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl fmt::Debug for PlatformSettingsRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_reload = self
            .x_bridge_reload_secret_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        let redacted_auth = self
            .x_auth_token_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        let redacted_bearer = self
            .x_bearer_token_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("PlatformSettingsRecord")
            .field("x_bridge_base_url", &self.x_bridge_base_url)
            .field("x_bridge_reload_url", &self.x_bridge_reload_url)
            .field("x_bridge_reload_secret_enc", &redacted_reload)
            .field(
                "x_bridge_reload_secret_hint",
                &self.x_bridge_reload_secret_hint,
            )
            .field("x_auth_token_enc", &redacted_auth)
            .field("x_auth_token_hint", &self.x_auth_token_hint)
            .field("x_bearer_token_enc", &redacted_bearer)
            .field("x_bearer_token_hint", &self.x_bearer_token_hint)
            .field("updated_by", &self.updated_by)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// Command to upsert singleton `platform_settings` row (redacted `Debug`).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct UpsertPlatformSettingsRecord {
    pub x_bridge_base_url: Option<String>,
    pub x_bridge_reload_url: Option<String>,
    pub x_bridge_reload_secret_enc: Option<String>,
    pub x_bridge_reload_secret_hint: Option<String>,
    pub x_auth_token_enc: Option<String>,
    pub x_auth_token_hint: Option<String>,
    pub x_bearer_token_enc: Option<String>,
    pub x_bearer_token_hint: Option<String>,
    pub updated_by: Option<Uuid>,
}

impl fmt::Debug for UpsertPlatformSettingsRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_reload = self
            .x_bridge_reload_secret_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        let redacted_auth = self
            .x_auth_token_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        let redacted_bearer = self
            .x_bearer_token_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("UpsertPlatformSettingsRecord")
            .field("x_bridge_base_url", &self.x_bridge_base_url)
            .field("x_bridge_reload_url", &self.x_bridge_reload_url)
            .field("x_bridge_reload_secret_enc", &redacted_reload)
            .field(
                "x_bridge_reload_secret_hint",
                &self.x_bridge_reload_secret_hint,
            )
            .field("x_auth_token_enc", &redacted_auth)
            .field("x_auth_token_hint", &self.x_auth_token_hint)
            .field("x_bearer_token_enc", &redacted_bearer)
            .field("x_bearer_token_hint", &self.x_bearer_token_hint)
            .field("updated_by", &self.updated_by)
            .finish()
    }
}

/// Non-secret platform integration configuration view (`GET`/`PUT /api/v1/admin/platform`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformSettingsView {
    pub x_bridge_base_url: Option<String>,
    pub x_bridge_reload_url: Option<String>,
    pub has_x_bridge_reload_secret: bool,
    pub x_bridge_reload_secret_hint: Option<String>,
    pub has_x_auth_token: bool,
    pub x_auth_token_hint: Option<String>,
    pub has_x_bearer_token: bool,
    pub x_bearer_token_hint: Option<String>,
    pub x_bridge_configured: bool,
    pub x_integration_ready: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

// ============================================================================
// Canonical Sources & User Subscriptions
// ============================================================================

/// Encrypted payload stored in `canonical_sources.config_json_enc`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceConfigPayload {
    pub handle_or_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<serde_json::Value>,
}

impl fmt::Debug for SourceConfigPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let creds = self
            .credentials
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("SourceConfigPayload")
            .field("handle_or_url", &self.handle_or_url)
            .field("credentials", &creds)
            .finish()
    }
}

/// Encrypts `SourceConfigPayload` into `config_json_enc`.
pub fn encode_source_config(
    cipher: &EnvelopeCipher,
    handle_or_url: &str,
    credentials: Option<serde_json::Value>,
) -> Result<String, CoreError> {
    let payload = SourceConfigPayload {
        handle_or_url: handle_or_url.trim().to_owned(),
        credentials,
    };
    let json = serde_json::to_string(&payload)
        .map_err(|err| CoreError::Internal(format!("failed to serialize source config: {err}")))?;
    cipher.encrypt(&json)
}

/// Decrypts `config_json_enc`, falling back to built-in preset `handle_or_url` when `None`.
pub fn decode_source_config(
    cipher: &EnvelopeCipher,
    canonical_key: &str,
    config_json_enc: Option<&str>,
) -> Result<SourceConfigPayload, CoreError> {
    if let Some(enc) = config_json_enc.map(str::trim).filter(|s| !s.is_empty()) {
        let raw = cipher.decrypt(enc)?;
        let parsed: SourceConfigPayload = serde_json::from_str(&raw).map_err(|err| {
            CoreError::Crypto(format!("invalid decrypted source config JSON: {err}"))
        })?;
        return Ok(parsed);
    }

    if let Some(preset) = find_preset_by_key(canonical_key) {
        return Ok(SourceConfigPayload {
            handle_or_url: preset.handle_or_url,
            credentials: None,
        });
    }

    Ok(SourceConfigPayload {
        handle_or_url: canonical_key.to_owned(),
        credentials: None,
    })
}

/// Produces a deterministic masked hint from optional source credentials JSON.
#[must_use]
pub fn mask_source_credentials_hint(credentials: Option<&serde_json::Value>) -> Option<String> {
    let obj = credentials?.as_object()?;
    if obj.is_empty() {
        return None;
    }
    for key in [
        "bearerToken",
        "bearer_token",
        "accessToken",
        "access_token",
        "token",
        "apiKey",
        "api_key",
    ] {
        if let Some(val) = obj
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(mask_secret(val));
        }
    }
    Some(SECRET_MASK_CONFIGURED_PLACEHOLDER.to_owned())
}

/// Internal canonical source record (`canonical_sources` table, redacted `Debug`).
#[derive(Clone, PartialEq, Eq)]
pub struct CanonicalSourceRecord {
    pub id: Uuid,
    pub source_type: SourceType,
    pub canonical_key: String,
    pub scope: SourceScope,
    pub scope_owner_id: Option<Uuid>,
    pub default_name: String,
    pub config_json_enc: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_fetched_at: Option<DateTime<Utc>>,
    pub next_poll_at: Option<DateTime<Utc>>,
    pub last_status: Option<FetchOutcome>,
    pub last_error: Option<String>,
    pub consecutive_failures: i32,
    pub created_at: DateTime<Utc>,
}

impl fmt::Debug for CanonicalSourceRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .config_json_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("CanonicalSourceRecord")
            .field("id", &self.id)
            .field("source_type", &self.source_type)
            .field("canonical_key", &self.canonical_key)
            .field("scope", &self.scope)
            .field("scope_owner_id", &self.scope_owner_id)
            .field("default_name", &self.default_name)
            .field("config_json_enc", &redacted)
            .field("last_status", &self.last_status)
            .field("consecutive_failures", &self.consecutive_failures)
            .finish()
    }
}

/// Conditional HTTP / poll scheduling update written after fetching a canonical source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFetchStateUpdate {
    pub outcome: FetchOutcome,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub fetched_at: DateTime<Utc>,
    pub next_poll_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub consecutive_failures: i32,
}

/// Non-secret subscribed source view returned to API and MCP clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSourceView {
    pub id: Uuid,
    pub subscription_id: Uuid,
    pub source_type: SourceType,
    pub scope: SourceScope,
    pub canonical_key: String,
    pub custom_name: String,
    pub default_name: String,
    pub category: String,
    pub handle_or_url: String,
    pub enabled: bool,
    pub has_credentials: bool,
    pub config_hint: Option<String>,
    pub last_fetched_at: Option<DateTime<Utc>>,
    pub next_poll_at: Option<DateTime<Utc>>,
    pub last_status: Option<FetchOutcome>,
    pub last_error: Option<String>,
    pub consecutive_failures: i32,
    pub created_at: DateTime<Utc>,
}

/// Internal joined view of `user_subscriptions` + `canonical_sources` (redacted `Debug`).
#[derive(Clone, PartialEq, Eq)]
pub struct UserSourceRecord {
    pub subscription_id: Uuid,
    pub canonical_source_id: Uuid,
    pub user_id: Uuid,
    pub source_type: SourceType,
    pub scope: SourceScope,
    pub scope_owner_id: Option<Uuid>,
    pub canonical_key: String,
    pub default_name: String,
    pub custom_name: String,
    pub category: String,
    pub enabled: bool,
    pub config_json_enc: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_fetched_at: Option<DateTime<Utc>>,
    pub next_poll_at: Option<DateTime<Utc>>,
    pub last_status: Option<FetchOutcome>,
    pub last_error: Option<String>,
    pub consecutive_failures: i32,
    pub created_at: DateTime<Utc>,
}

impl fmt::Debug for UserSourceRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .config_json_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("UserSourceRecord")
            .field("subscription_id", &self.subscription_id)
            .field("canonical_source_id", &self.canonical_source_id)
            .field("user_id", &self.user_id)
            .field("source_type", &self.source_type)
            .field("scope", &self.scope)
            .field("canonical_key", &self.canonical_key)
            .field("custom_name", &self.custom_name)
            .field("enabled", &self.enabled)
            .field("config_json_enc", &redacted)
            .finish()
    }
}

impl UserSourceRecord {
    /// Converts this internal joined record into a non-secret `UserSourceView`.
    pub fn to_view(&self, cipher: &EnvelopeCipher) -> Result<UserSourceView, CoreError> {
        let decoded =
            decode_source_config(cipher, &self.canonical_key, self.config_json_enc.as_deref())?;
        let config_hint = mask_source_credentials_hint(decoded.credentials.as_ref());
        let has_credentials = config_hint.is_some();
        Ok(UserSourceView {
            id: self.canonical_source_id,
            subscription_id: self.subscription_id,
            source_type: self.source_type,
            scope: self.scope,
            canonical_key: self.canonical_key.clone(),
            custom_name: self.custom_name.clone(),
            default_name: self.default_name.clone(),
            category: self.category.clone(),
            handle_or_url: decoded.handle_or_url,
            enabled: self.enabled,
            has_credentials,
            config_hint,
            last_fetched_at: self.last_fetched_at,
            next_poll_at: self.next_poll_at,
            last_status: self.last_status,
            last_error: self.last_error.clone(),
            consecutive_failures: self.consecutive_failures,
            created_at: self.created_at,
        })
    }
}

/// Command to subscribe to or create a canonical source (`shared_public` or `user_private`).
#[derive(Clone, PartialEq, Eq)]
pub struct SubscribeSourceCmd {
    pub source_type: SourceType,
    pub scope: SourceScope,
    pub canonical_key: String,
    pub custom_name: String,
    pub default_name: String,
    pub category: String,
    pub handle_or_url: String,
    pub config_json_enc: Option<String>,
    pub enabled: bool,
}

impl fmt::Debug for SubscribeSourceCmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .config_json_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("SubscribeSourceCmd")
            .field("source_type", &self.source_type)
            .field("scope", &self.scope)
            .field("canonical_key", &self.canonical_key)
            .field("custom_name", &self.custom_name)
            .field("category", &self.category)
            .field("handle_or_url", &self.handle_or_url)
            .field("config_json_enc", &redacted)
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl SubscribeSourceCmd {
    /// Resolves the `scope_owner_id` enforced for this source's `SourceScope`.
    #[must_use]
    pub const fn scope_owner_id(&self, user_id: Uuid) -> Option<Uuid> {
        match self.scope {
            SourceScope::SharedPublic => None,
            SourceScope::UserPrivate => Some(user_id),
        }
    }
}

/// Command to update a user's source subscription (and private source config when `user_private`).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct UpdateSourceCmd {
    pub custom_name: Option<String>,
    pub category: Option<String>,
    pub enabled: Option<bool>,
    pub handle_or_url: Option<String>,
    pub config_json_enc: Option<String>,
}

impl fmt::Debug for UpdateSourceCmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .config_json_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("UpdateSourceCmd")
            .field("custom_name", &self.custom_name)
            .field("category", &self.category)
            .field("enabled", &self.enabled)
            .field("handle_or_url", &self.handle_or_url)
            .field("config_json_enc", &redacted)
            .finish()
    }
}

/// Non-secret source specification used for JSON export/import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedSourceSpec {
    pub source_type: SourceType,
    pub scope: SourceScope,
    pub canonical_key: String,
    pub custom_name: String,
    pub category: String,
    pub handle_or_url: String,
    pub enabled: bool,
}

/// Portable non-secret source export bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceExportBundle {
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub sources: Vec<ExportedSourceSpec>,
}

// ============================================================================
// Canonical Items, Search, Matches & Bookmarks
// ============================================================================

/// Persisted canonical item (`canonical_items` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalItemRecord {
    pub id: Uuid,
    pub canonical_source_id: Uuid,
    pub external_id: Option<String>,
    pub canonical_url: String,
    pub content_hash: String,
    pub title: String,
    pub author: Option<String>,
    pub summary_raw: Option<String>,
    pub body_pruned: bool,
    pub published_at: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
}

/// New collected item candidate for `ON CONFLICT DO NOTHING` insertion into `canonical_items`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCanonicalItem {
    pub external_id: Option<String>,
    pub canonical_url: String,
    pub content_hash: String,
    pub title: String,
    pub author: Option<String>,
    pub summary_raw: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
}

/// User-scoped item view combining canonical item, source metadata, matches, and bookmark state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    pub id: Uuid,
    pub source_id: Uuid,
    pub source_name: String,
    pub category: String,
    pub external_id: Option<String>,
    pub canonical_url: String,
    pub title: String,
    pub author: Option<String>,
    pub summary: Option<String>,
    pub body_pruned: bool,
    pub published_at: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub matched_keywords: Vec<String>,
    pub priority: AlertPriority,
    pub importance_score: i32,
    pub bookmarked: bool,
    pub bookmark_note: Option<String>,
}

/// Full-text search and filtering parameters for `GET /api/v1/items`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemSearchQuery {
    pub q: Option<String>,
    pub source_id: Option<Uuid>,
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
    pub keyword: Option<String>,
    pub bookmarked: Option<bool>,
    pub page: PageRequest,
}

/// Watchlist match outcome recorded per `(user_id, item_id)` in `user_item_matches`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUserItemMatch {
    pub item_id: Uuid,
    pub matched_keywords: Vec<String>,
    pub priority: AlertPriority,
    pub importance_score: i32,
}

/// Unalerted urgent item ready for immediate alert dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingAlertItem {
    pub item_id: Uuid,
    pub source_id: Uuid,
    pub source_name: String,
    pub category: String,
    pub canonical_url: String,
    pub title: String,
    pub author: Option<String>,
    pub summary: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub matched_keywords: Vec<String>,
    pub priority: AlertPriority,
    pub importance_score: i32,
}

/// Saved user bookmark (`user_bookmarks` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkView {
    pub id: Uuid,
    pub item_id: Uuid,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// Daily Digests & Per-Source Digests
// ============================================================================

/// Summary row returned by `GET /api/v1/digests`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyDigestSummary {
    pub id: Uuid,
    pub digest_date: NaiveDate,
    pub title: String,
    pub item_count: i32,
    pub highlighted_count: i32,
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub model_used: String,
    pub degraded_fallback: bool,
    pub created_at: DateTime<Utc>,
}

/// Full daily digest returned by `GET /api/v1/digests/{date}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyDigestDetail {
    pub id: Uuid,
    pub digest_date: NaiveDate,
    pub input_fingerprint: String,
    pub title: String,
    pub overview_markdown: String,
    pub overview_html: String,
    pub sections: Vec<DigestSection>,
    pub item_count: i32,
    pub highlighted_count: i32,
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub model_used: String,
    pub degraded_fallback: bool,
    pub created_at: DateTime<Utc>,
}

/// Independent per-source daily digest view returned by `GET /api/v1/sources/{id}/digests`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDailyDigestView {
    pub id: Uuid,
    pub digest_id: Option<Uuid>,
    pub source_id: Uuid,
    pub source_name: String,
    pub digest_date: NaiveDate,
    pub input_fingerprint: String,
    pub summary_markdown: String,
    pub key_points: Vec<String>,
    pub item_count: i32,
    pub created_at: DateTime<Utc>,
}

/// Per-source summary payload inside `SaveDigestCmd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSourceDigestCmd {
    pub canonical_source_id: Uuid,
    pub digest_date: NaiveDate,
    pub input_fingerprint: String,
    pub summary_markdown: String,
    pub key_points: Vec<String>,
    pub item_count: i32,
}

/// Atomic command to upsert a daily digest and its per-source daily summaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveDigestCmd {
    pub user_id: Uuid,
    pub digest_date: NaiveDate,
    pub input_fingerprint: String,
    pub title: String,
    pub overview_markdown: String,
    pub overview_html: String,
    pub sections: Vec<DigestSection>,
    pub item_count: i32,
    pub highlighted_count: i32,
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub model_used: String,
    pub degraded_fallback: bool,
    pub source_digests: Vec<SaveSourceDigestCmd>,
}

// ============================================================================
// Notification Channels, Schedules & Watchlist Rules
// ============================================================================

/// Internal persisted notification channel (`user_notification_channels` table, redacted `Debug`).
#[derive(Clone, PartialEq, Eq)]
pub struct NotificationChannelRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub channel_type: NotificationChannelType,
    pub name: String,
    pub enabled: bool,
    pub config_enc: String,
    pub config_hint: String,
    pub trigger_on_daily: bool,
    pub trigger_on_alert: bool,
    pub created_at: DateTime<Utc>,
}

impl fmt::Debug for NotificationChannelRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotificationChannelRecord")
            .field("id", &self.id)
            .field("user_id", &self.user_id)
            .field("channel_type", &self.channel_type)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("config_enc", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .field("config_hint", &self.config_hint)
            .field("trigger_on_daily", &self.trigger_on_daily)
            .field("trigger_on_alert", &self.trigger_on_alert)
            .field("created_at", &self.created_at)
            .finish()
    }
}

impl NotificationChannelRecord {
    /// Converts this persisted channel record into a masked API view.
    #[must_use]
    pub fn to_view(&self) -> NotificationChannelView {
        NotificationChannelView {
            id: self.id,
            channel_type: self.channel_type,
            name: self.name.clone(),
            enabled: self.enabled,
            config_hint: self.config_hint.clone(),
            trigger_on_daily: self.trigger_on_daily,
            trigger_on_alert: self.trigger_on_alert,
            created_at: self.created_at,
        }
    }
}

/// Masked notification channel view returned by `/api/v1/settings/channels`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationChannelView {
    pub id: Uuid,
    pub channel_type: NotificationChannelType,
    pub name: String,
    pub enabled: bool,
    pub config_hint: String,
    pub trigger_on_daily: bool,
    pub trigger_on_alert: bool,
    pub created_at: DateTime<Utc>,
}

/// Command to insert a validated, encrypted notification channel.
#[derive(Clone, PartialEq, Eq)]
pub struct CreateChannelRecord {
    pub channel_type: NotificationChannelType,
    pub name: String,
    pub enabled: bool,
    pub config_enc: String,
    pub config_hint: String,
    pub trigger_on_daily: bool,
    pub trigger_on_alert: bool,
}

impl fmt::Debug for CreateChannelRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateChannelRecord")
            .field("channel_type", &self.channel_type)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("config_enc", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .field("config_hint", &self.config_hint)
            .field("trigger_on_daily", &self.trigger_on_daily)
            .field("trigger_on_alert", &self.trigger_on_alert)
            .finish()
    }
}

/// Command to update an existing notification channel.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct UpdateChannelRecord {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub config_enc: Option<String>,
    pub config_hint: Option<String>,
    pub trigger_on_daily: Option<bool>,
    pub trigger_on_alert: Option<bool>,
}

impl fmt::Debug for UpdateChannelRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .config_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("UpdateChannelRecord")
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("config_enc", &redacted)
            .field("config_hint", &self.config_hint)
            .field("trigger_on_daily", &self.trigger_on_daily)
            .field("trigger_on_alert", &self.trigger_on_alert)
            .finish()
    }
}

/// Persisted user schedule (`user_schedules` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserScheduleRecord {
    pub user_id: Uuid,
    pub timezone: String,
    pub daily_time: String,
    pub email_enabled: bool,
    pub send_when_empty: bool,
    pub alert_scan_enabled: bool,
    pub alert_interval_minutes: i32,
    pub last_daily_run_date: Option<NaiveDate>,
    pub last_alert_scan_at: Option<DateTime<Utc>>,
}

/// Command to update a user's schedule configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateScheduleCmd {
    pub timezone: String,
    pub daily_time: String,
    pub email_enabled: bool,
    pub send_when_empty: bool,
    pub alert_scan_enabled: bool,
    pub alert_interval_minutes: i32,
}

impl UpdateScheduleCmd {
    /// Validates IANA timezone, `HH:MM` daily time, and positive alert scan interval.
    pub fn validate(&self) -> Result<(), CoreError> {
        let tz = self.timezone.trim();
        tz.parse::<chrono_tz::Tz>().map_err(|_| {
            CoreError::InvalidInput(format!("invalid IANA timezone: {}", self.timezone))
        })?;

        let time_str = self.daily_time.trim();
        let (hh, mm) = time_str.split_once(':').ok_or_else(|| {
            CoreError::InvalidInput("dailyTime must be in HH:MM 24-hour format".to_owned())
        })?;
        if hh.len() != 2 || mm.len() != 2 {
            return Err(CoreError::InvalidInput(
                "dailyTime must be in HH:MM 24-hour format".to_owned(),
            ));
        }
        let hour: u8 = hh.parse().map_err(|_| {
            CoreError::InvalidInput("dailyTime hour must be numeric (00..23)".to_owned())
        })?;
        let minute: u8 = mm.parse().map_err(|_| {
            CoreError::InvalidInput("dailyTime minute must be numeric (00..59)".to_owned())
        })?;
        if hour > 23 || minute > 59 {
            return Err(CoreError::InvalidInput(
                "dailyTime must be between 00:00 and 23:59".to_owned(),
            ));
        }

        if self.alert_interval_minutes <= 0 {
            return Err(CoreError::InvalidInput(
                "alertIntervalMinutes must be >= 1".to_owned(),
            ));
        }

        Ok(())
    }
}

/// Persisted watchlist rule (`user_watchlist_rules` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistRuleRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub keywords: Vec<String>,
    pub exclude_keywords: Vec<String>,
    pub priority: AlertPriority,
    pub notify_immediately: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

impl WatchlistRuleRecord {
    /// Converts a persisted watchlist rule into `digest_core`'s pipeline rule input.
    #[must_use]
    pub fn to_core_input(&self) -> WatchlistRuleInput {
        WatchlistRuleInput {
            id: Some(self.id),
            name: self.name.clone(),
            keywords: self.keywords.clone(),
            exclude_keywords: self.exclude_keywords.clone(),
            priority: self.priority,
            notify_immediately: self.notify_immediately,
            enabled: self.enabled,
        }
    }
}

/// Command to create or update a watchlist rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertWatchlistRuleCmd {
    pub name: String,
    pub keywords: Vec<String>,
    pub exclude_keywords: Vec<String>,
    pub priority: AlertPriority,
    pub notify_immediately: bool,
    pub enabled: bool,
}

// ============================================================================
// Runs, Delivery Logs & Statistics
// ============================================================================

/// Persisted digest/alert execution run (`digest_runs` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestRunRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub run_type: RunType,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub items_fetched: i32,
    pub items_new: i32,
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub error_summary: Option<String>,
}

/// Persisted notification delivery attempt (`delivery_logs` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryLogRecord {
    pub id: Uuid,
    pub run_id: Uuid,
    pub user_id: Uuid,
    pub digest_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub channel_type: NotificationChannelType,
    pub status: DeliveryStatus,
    pub error_message: Option<String>,
    pub sent_at: DateTime<Utc>,
}

/// Execution run combined with its child delivery logs for `GET /api/v1/runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestRunView {
    pub id: Uuid,
    pub run_type: RunType,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub items_fetched: i32,
    pub items_new: i32,
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub error_summary: Option<String>,
    pub deliveries: Vec<DeliveryLogRecord>,
}

/// Command to start a new run record (`status = running`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRunCmd {
    pub user_id: Uuid,
    pub run_type: RunType,
    pub started_at: DateTime<Utc>,
}

/// Command to finalize an execution run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishRunCmd {
    pub status: RunStatus,
    pub finished_at: DateTime<Utc>,
    pub items_fetched: i32,
    pub items_new: i32,
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub error_summary: Option<String>,
}

/// Command to record a delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDeliveryLogCmd {
    pub run_id: Uuid,
    pub user_id: Uuid,
    pub digest_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub channel_type: NotificationChannelType,
    pub status: DeliveryStatus,
    pub error_message: Option<String>,
    pub sent_at: DateTime<Utc>,
}

/// Per-source health summary included in `RunStatsView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceHealthStat {
    pub source_id: Uuid,
    pub source_name: String,
    pub source_type: SourceType,
    pub scope: SourceScope,
    pub enabled: bool,
    pub last_status: Option<FetchOutcome>,
    pub last_fetched_at: Option<DateTime<Utc>>,
    pub consecutive_failures: i32,
    pub last_error: Option<String>,
}

/// Aggregated user execution, token, and delivery metrics (`GET /api/v1/runs/stats`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStatsView {
    pub total_runs: i64,
    pub succeeded_runs: i64,
    pub failed_runs: i64,
    pub total_prompt_tokens: i64,
    pub total_completion_tokens: i64,
    pub deliveries_sent: i64,
    pub deliveries_failed: i64,
    pub source_health: Vec<SourceHealthStat>,
}

#[cfg(test)]
mod tests {
    use digest_core::constants::CRYPTO_KEY_BYTE_LEN;

    use super::*;

    #[test]
    fn page_request_validates_bounds_and_computes_offset() {
        assert!(matches!(
            PageRequest::new(Some(0), None),
            Err(CoreError::InvalidInput(_))
        ));
        assert!(matches!(
            PageRequest::new(Some(1), Some(0)),
            Err(CoreError::InvalidInput(_))
        ));

        let page = PageRequest::new(Some(3), Some(500)).unwrap();
        assert_eq!(page.page, 3);
        assert_eq!(page.page_size, DEFAULT_MAX_PAGE_SIZE);
        assert_eq!(page.offset(), 2 * DEFAULT_MAX_PAGE_SIZE);
    }

    #[test]
    fn sensitive_records_redact_secrets_in_debug() {
        let cipher = EnvelopeCipher::from_bytes([0x33u8; CRYPTO_KEY_BYTE_LEN]);
        let enc_ai = cipher.encrypt("test-secret-ai-key").unwrap();
        let ai_rec = UserAiConfigRecord {
            user_id: Uuid::new_v4(),
            provider_label: "DeepSeek".to_owned(),
            base_url: "https://api.deepseek.com".to_owned(),
            api_key_enc: Some(enc_ai.clone()),
            api_key_hint: Some("test****-key".to_owned()),
            model: "deepseek-chat".to_owned(),
            system_prompt: "prompt".to_owned(),
            temperature: 0.2,
            max_tokens: 4096,
            updated_at: Utc::now(),
        };
        assert!(!format!("{ai_rec:?}").contains(&enc_ai));

        let chan = NotificationChannelRecord {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            channel_type: NotificationChannelType::Webhook,
            name: "Hook".to_owned(),
            enabled: true,
            config_enc: "v1:secret-ciphertext".to_owned(),
            config_hint: "https://example.com/****".to_owned(),
            trigger_on_daily: true,
            trigger_on_alert: true,
            created_at: Utc::now(),
        };
        assert!(!format!("{chan:?}").contains("v1:secret-ciphertext"));
    }

    #[test]
    fn source_config_roundtrip_and_preset_fallback_work() {
        let cipher = EnvelopeCipher::from_bytes([0x44u8; CRYPTO_KEY_BYTE_LEN]);
        let enc = encode_source_config(
            &cipher,
            "https://example.com/feed.xml",
            Some(serde_json::json!({ "bearerToken": "test-token-value-1234" })),
        )
        .unwrap();

        let decoded = decode_source_config(&cipher, "custom-rss", Some(&enc)).unwrap();
        assert_eq!(decoded.handle_or_url, "https://example.com/feed.xml");
        assert_eq!(
            mask_source_credentials_hint(decoded.credentials.as_ref()).as_deref(),
            Some("test****1234")
        );

        let fallback = decode_source_config(&cipher, "google-ai-blog", None).unwrap();
        assert!(fallback.handle_or_url.starts_with("https://"));
        assert!(fallback.credentials.is_none());
    }

    #[test]
    fn update_schedule_cmd_validates_timezone_time_and_interval() {
        let valid = UpdateScheduleCmd {
            timezone: "Asia/Shanghai".to_owned(),
            daily_time: "08:30".to_owned(),
            email_enabled: true,
            send_when_empty: false,
            alert_scan_enabled: true,
            alert_interval_minutes: 60,
        };
        assert!(valid.validate().is_ok());

        let bad_tz = UpdateScheduleCmd {
            timezone: "Mars/Olympus".to_owned(),
            ..valid.clone()
        };
        assert!(matches!(bad_tz.validate(), Err(CoreError::InvalidInput(_))));

        let bad_time = UpdateScheduleCmd {
            daily_time: "25:00".to_owned(),
            ..valid.clone()
        };
        assert!(matches!(
            bad_time.validate(),
            Err(CoreError::InvalidInput(_))
        ));

        let bad_interval = UpdateScheduleCmd {
            alert_interval_minutes: 0,
            ..valid
        };
        assert!(matches!(
            bad_interval.validate(),
            Err(CoreError::InvalidInput(_))
        ));
    }
}
