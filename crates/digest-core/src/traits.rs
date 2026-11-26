//! Core extension traits locking in Strategy, Pipeline, and Notifier
//! boundaries for downstream crates.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{
    AlertPriority, DailyDigestSynthesisInput, FetchOutcome, NotificationChannelType, ProcessedItem,
    RenderInput, SourceDigestInput, SourceScope, SourceSummaryResult, SourceType, UserAiConfig,
    WatchlistRuleInput,
};
use crate::error::CoreError;

/// Descriptor passed to a `SourceCollector`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDescriptor {
    pub id: Uuid,
    pub source_type: SourceType,
    pub scope: SourceScope,
    pub canonical_key: String,
    pub name: String,
    pub handle_or_url: String,
    pub category: String,
}

/// Conditional HTTP headers attached to upstream requests for 304 bandwidth savings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionalFetchHeaders {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Normalized raw item returned by an upstream collector.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCollectedItem {
    pub external_id: Option<String>,
    pub url: String,
    pub title: String,
    pub author: Option<String>,
    pub summary: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
}

/// Outcome and optional items returned by a `SourceCollector`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchResult {
    pub outcome: FetchOutcome,
    pub items: Vec<RawCollectedItem>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Strategy trait implemented by each upstream collector (`rss`, `github_release`, etc.).
#[async_trait]
pub trait SourceCollector: Send + Sync {
    /// Returns the `SourceType` handled by this collector.
    fn source_type(&self) -> SourceType;

    /// Fetches normalized candidate items using conditional HTTP headers when available.
    async fn fetch(
        &self,
        source: &SourceDescriptor,
        conditional: &ConditionalFetchHeaders,
    ) -> Result<FetchResult, CoreError>;

    /// Fetches normalized candidate items using optional decrypted source credentials.
    async fn fetch_with_credentials(
        &self,
        source: &SourceDescriptor,
        conditional: &ConditionalFetchHeaders,
        _credentials_json: Option<&serde_json::Value>,
    ) -> Result<FetchResult, CoreError> {
        self.fetch(source, conditional).await
    }
}

/// Pipeline step applied to a batch of collected items (normalization, dedup, scoring).
#[async_trait]
pub trait ItemProcessor: Send + Sync {
    /// Processes or filters candidate items in place or returns the transformed set.
    async fn process(
        &self,
        items: Vec<RawCollectedItem>,
    ) -> Result<Vec<RawCollectedItem>, CoreError>;
}

/// Extended item processing pipeline supporting source context and watchlist rules.
#[async_trait]
pub trait StructuredItemProcessor: ItemProcessor {
    /// Normalizes, deduplicates, matches watchlist rules, and scores items.
    async fn process_items(
        &self,
        source: Option<&SourceDescriptor>,
        items: Vec<RawCollectedItem>,
        watchlist_rules: &[WatchlistRuleInput],
    ) -> Result<Vec<ProcessedItem>, CoreError>;
}

/// Output produced by an LLM summarizer or deterministic fallback.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummarizedDigest {
    pub title: String,
    pub overview_markdown: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub model_used: String,
    pub degraded_fallback: bool,
}

/// Single model descriptor returned by an OpenAI-compatible `/models` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModelOption {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
}

/// Strategy trait implemented by LLM providers and deterministic fallbacks.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Generates structured summary output for a batch of collected items.
    async fn summarize(
        &self,
        system_prompt: &str,
        items: &[RawCollectedItem],
    ) -> Result<SummarizedDigest, CoreError>;
}

/// Extended LLM summarization strategy supporting per-source summaries,
/// daily synthesis, explicit `UserAiConfig`, and connectivity checks.
#[async_trait]
pub trait StructuredLlmProvider: LlmProvider {
    /// Summarizes a single source's items for a digest date.
    async fn summarize_source(
        &self,
        config: &UserAiConfig,
        input: &SourceDigestInput,
    ) -> Result<SourceSummaryResult, CoreError>;

    /// Synthesizes the global daily digest across per-source summaries and highlights.
    async fn synthesize_daily(
        &self,
        config: &UserAiConfig,
        input: &DailyDigestSynthesisInput,
    ) -> Result<SummarizedDigest, CoreError>;

    /// Verifies connectivity and credentials against the configured AI endpoint.
    async fn test_connection(&self, config: &UserAiConfig) -> Result<(), CoreError>;

    /// Lists available model identifiers from the configured AI endpoint.
    async fn list_models(&self, _config: &UserAiConfig) -> Result<Vec<AiModelOption>, CoreError> {
        Ok(Vec::new())
    }
}

/// Rendered digest payload ready for email or IM delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedNotification {
    pub subject: String,
    pub text_body: String,
    pub html_body: String,
    pub markdown_body: String,
    pub priority: AlertPriority,
}

/// Strategy trait for rendering digests across formats.
pub trait DigestRenderer: Send + Sync {
    /// Converts a summarized digest and items into multi-format output.
    fn render(
        &self,
        digest: &SummarizedDigest,
        items: &[RawCollectedItem],
        priority: AlertPriority,
    ) -> Result<RenderedNotification, CoreError>;

    /// Converts structured sections, highlights, and digest metadata into multi-format output.
    fn render_structured(&self, input: &RenderInput) -> Result<RenderedNotification, CoreError> {
        let raw_items: Vec<RawCollectedItem> = input
            .all_items
            .iter()
            .map(ProcessedItem::to_raw_item)
            .collect();
        self.render(&input.digest, &raw_items, input.priority)
    }
}

/// Strategy trait implemented by each notification channel (`smtp`, `telegram`, etc.).
#[async_trait]
pub trait Notifier: Send + Sync {
    /// Returns the channel type handled by this notifier.
    fn channel_type(&self) -> NotificationChannelType;

    /// Dispatches a rendered notification using channel-specific decrypted config JSON.
    async fn send(
        &self,
        channel_config_json: &serde_json::Value,
        payload: &RenderedNotification,
    ) -> Result<(), CoreError>;
}
