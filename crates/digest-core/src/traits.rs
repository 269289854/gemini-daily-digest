//! Core extension traits locking in Strategy, Pipeline, and Notifier
//! boundaries for downstream crates.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{
    AlertPriority, FetchOutcome, NotificationChannelType, SourceScope, SourceType,
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
