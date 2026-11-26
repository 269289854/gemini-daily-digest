//! Strongly typed shared domain models exchanged across collectors,
//! processing pipeline, LLM summarizers, renderers, notifiers, and workflows.

use std::fmt;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::constants::{
    DEFAULT_AI_BASE_URL, DEFAULT_AI_MODEL, DEFAULT_AI_PROVIDER_LABEL, DEFAULT_LLM_MAX_TOKENS,
    DEFAULT_LLM_SYSTEM_PROMPT, DEFAULT_LLM_TEMPERATURE, SECRET_MASK_REDACTED_PLACEHOLDER,
};
use crate::domain::enums::{
    AlertPriority, DeliveryStatus, FetchOutcome, NotificationChannelType, RunStatus, RunType,
    SourceScope, SourceType,
};
use crate::traits::{
    ConditionalFetchHeaders, RawCollectedItem, RenderedNotification, SourceDescriptor,
    SummarizedDigest,
};

/// Built-in preset source specification (Gemini Ecosystem Preset Pack).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetSourceSpec {
    pub canonical_key: String,
    pub source_type: SourceType,
    pub scope: SourceScope,
    pub default_name: String,
    pub category: String,
    pub handle_or_url: String,
    pub description: String,
    pub requires_user_credentials: bool,
}

/// User-defined keyword watchlist rule evaluated during item processing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistRuleInput {
    pub id: Option<Uuid>,
    pub name: String,
    pub keywords: Vec<String>,
    pub exclude_keywords: Vec<String>,
    pub priority: AlertPriority,
    pub notify_immediately: bool,
    pub enabled: bool,
}

/// Result of matching a collected item against a user's watchlist rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemMatchResult {
    pub matched_keywords: Vec<String>,
    pub priority: AlertPriority,
    pub importance_score: i32,
    pub notify_immediately: bool,
}

impl Default for ItemMatchResult {
    fn default() -> Self {
        Self {
            matched_keywords: Vec::new(),
            priority: AlertPriority::Normal,
            importance_score: 0,
            notify_immediately: false,
        }
    }
}

/// Canonicalized, deduplicated, and scored item produced by `ItemProcessingPipeline`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessedItem {
    pub external_id: Option<String>,
    pub canonical_url: String,
    pub content_hash: String,
    pub title: String,
    pub author: Option<String>,
    pub summary: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub source_id: Option<Uuid>,
    pub source_name: Option<String>,
    pub category: Option<String>,
    pub match_result: ItemMatchResult,
}

impl ProcessedItem {
    /// Converts this processed item into a `RawCollectedItem` view for Round 0 interfaces.
    #[must_use]
    pub fn to_raw_item(&self) -> RawCollectedItem {
        RawCollectedItem {
            external_id: self.external_id.clone(),
            url: self.canonical_url.clone(),
            title: self.title.clone(),
            author: self.author.clone(),
            summary: self.summary.clone(),
            published_at: self.published_at,
        }
    }
}

impl From<&ProcessedItem> for RawCollectedItem {
    fn from(item: &ProcessedItem) -> Self {
        item.to_raw_item()
    }
}

/// Runtime OpenAI-compatible AI configuration for a user (with redacted `Debug`).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserAiConfig {
    pub provider_label: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub system_prompt: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

impl Default for UserAiConfig {
    fn default() -> Self {
        Self {
            provider_label: DEFAULT_AI_PROVIDER_LABEL.to_owned(),
            base_url: DEFAULT_AI_BASE_URL.to_owned(),
            api_key: None,
            model: DEFAULT_AI_MODEL.to_owned(),
            system_prompt: DEFAULT_LLM_SYSTEM_PROMPT.to_owned(),
            temperature: DEFAULT_LLM_TEMPERATURE,
            max_tokens: DEFAULT_LLM_MAX_TOKENS,
        }
    }
}

impl fmt::Debug for UserAiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let masked_key = self
            .api_key
            .as_ref()
            .filter(|value| !value.is_empty())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("UserAiConfig")
            .field("provider_label", &self.provider_label)
            .field("base_url", &self.base_url)
            .field("api_key", &masked_key)
            .field("model", &self.model)
            .field("system_prompt", &self.system_prompt)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .finish()
    }
}

/// Input to summarize a single subscribed source's collected items for a digest date.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDigestInput {
    pub source: SourceDescriptor,
    pub digest_date: NaiveDate,
    pub items: Vec<ProcessedItem>,
}

/// Independent per-source summary produced by an LLM provider or deterministic fallback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSummaryResult {
    pub source_id: Uuid,
    pub source_name: String,
    pub category: String,
    pub digest_date: NaiveDate,
    pub input_fingerprint: String,
    pub summary_markdown: String,
    pub key_points: Vec<String>,
    pub item_count: usize,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub model_used: String,
    pub degraded_fallback: bool,
}

/// Input to synthesize a global daily digest across per-source summaries and highlights.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyDigestSynthesisInput {
    pub digest_date: NaiveDate,
    pub source_summaries: Vec<SourceSummaryResult>,
    pub highlighted_items: Vec<ProcessedItem>,
    pub all_items: Vec<ProcessedItem>,
}

/// Categorized digest section rendered into HTML, Markdown, and plain-text outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestSection {
    pub category: String,
    pub source_id: Option<Uuid>,
    pub source_name: String,
    pub summary_markdown: String,
    pub key_points: Vec<String>,
    pub items: Vec<ProcessedItem>,
}

/// Structured input passed to `DigestRenderer::render_structured`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderInput {
    pub digest_date: NaiveDate,
    pub digest: SummarizedDigest,
    pub sections: Vec<DigestSection>,
    pub highlighted_items: Vec<ProcessedItem>,
    pub all_items: Vec<ProcessedItem>,
    pub priority: AlertPriority,
}

/// Decrypted outbound notifier configuration (with redacted `Debug`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifierConfig {
    pub channel_id: Option<Uuid>,
    pub channel_type: NotificationChannelType,
    pub name: String,
    pub config_json: serde_json::Value,
}

impl fmt::Debug for NotifierConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotifierConfig")
            .field("channel_id", &self.channel_id)
            .field("channel_type", &self.channel_type)
            .field("name", &self.name)
            .field("config_json", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .finish()
    }
}

/// Per-channel notification dispatch result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryOutcome {
    pub channel_id: Option<Uuid>,
    pub channel_type: NotificationChannelType,
    pub status: DeliveryStatus,
    pub error_message: Option<String>,
    pub sent_at: DateTime<Utc>,
}

/// Source descriptor paired with conditional HTTP state and optional decrypted credentials.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRunInput {
    pub source: SourceDescriptor,
    pub conditional: ConditionalFetchHeaders,
    pub credentials_json: Option<serde_json::Value>,
}

impl fmt::Debug for SourceRunInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let has_creds = self
            .credentials_json
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("SourceRunInput")
            .field("source", &self.source)
            .field("conditional", &self.conditional)
            .field("credentials_json", &has_creds)
            .finish()
    }
}

/// Fetch status snapshot recorded for each source during a workflow run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFetchRecord {
    pub source_id: Uuid,
    pub outcome: FetchOutcome,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub items_fetched: usize,
    pub error_message: Option<String>,
}

/// Complete input specification for `DigestExecutionWorkflow`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestWorkflowInput {
    pub user_id: Option<Uuid>,
    pub digest_date: NaiveDate,
    pub run_type: RunType,
    pub sources: Vec<SourceRunInput>,
    pub watchlist_rules: Vec<WatchlistRuleInput>,
    pub ai_config: UserAiConfig,
    pub notifiers: Vec<NotifierConfig>,
    pub send_when_empty: bool,
    pub force_refresh: bool,
}

/// Aggregated result produced by `DigestExecutionWorkflow`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestWorkflowOutput {
    pub status: RunStatus,
    pub digest_date: NaiveDate,
    pub input_fingerprint: String,
    pub source_fetches: Vec<SourceFetchRecord>,
    pub processed_items: Vec<ProcessedItem>,
    pub source_summaries: Vec<SourceSummaryResult>,
    pub summarized_digest: SummarizedDigest,
    pub sections: Vec<DigestSection>,
    pub rendered: Option<RenderedNotification>,
    pub deliveries: Vec<DeliveryOutcome>,
    pub items_fetched: usize,
    pub items_new: usize,
    pub highlighted_count: usize,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub cache_hit: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_ai_config_and_notifier_config_redact_secrets_in_debug() {
        let ai = UserAiConfig {
            api_key: Some("super-secret-api-key-value".to_owned()),
            ..UserAiConfig::default()
        };
        let ai_debug = format!("{ai:?}");
        assert!(!ai_debug.contains("super-secret-api-key-value"));
        assert!(ai_debug.contains(SECRET_MASK_REDACTED_PLACEHOLDER));

        let notifier = NotifierConfig {
            channel_id: None,
            channel_type: NotificationChannelType::Webhook,
            name: "test".to_owned(),
            config_json: serde_json::json!({ "secret": "hidden-token-value" }),
        };
        let notifier_debug = format!("{notifier:?}");
        assert!(!notifier_debug.contains("hidden-token-value"));
        assert!(notifier_debug.contains(SECRET_MASK_REDACTED_PLACEHOLDER));
    }
}
