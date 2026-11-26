//! Server-level service abstractions for authentication, real-time SSE
//! publishing, and API-triggered execution/testing orchestration.

use std::fmt;
use std::net::SocketAddr;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use digest_core::constants::SECRET_MASK_REDACTED_PLACEHOLDER;
use digest_core::domain::{DeliveryStatus, FetchOutcome, NotificationChannelType, SseEventType};
use digest_core::error::CoreError;
use http::HeaderMap;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::repo::{DeliveryLogRecord, DigestRunView, UserRecord};

/// Verified caller identity extracted from OIDC JWT claims or loopback `dev_bypass`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthIdentity {
    pub oidc_sub: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub role: String,
}

/// Strategy trait authenticating incoming HTTP and SSE requests.
#[async_trait]
pub trait AuthService: Send + Sync {
    async fn authenticate_bearer_or_bypass(
        &self,
        headers: &HeaderMap,
        peer_addr: Option<SocketAddr>,
    ) -> Result<UserRecord, CoreError>;
}

/// Real-time Server-Sent Event envelope published per user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SseEnvelope {
    pub event_type: SseEventType,
    pub payload: serde_json::Value,
}

/// Broadcast hub trait publishing and subscribing to per-user SSE envelopes.
pub trait SsePublisher: Send + Sync {
    fn publish(&self, user_id: Uuid, event: SseEnvelope);
    fn subscribe(&self, user_id: Uuid) -> broadcast::Receiver<SseEnvelope>;
}

/// Request payload for `POST /api/v1/runs/trigger`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerRunRequest {
    pub digest_date: Option<NaiveDate>,
    #[serde(default)]
    pub force_refresh: bool,
    #[serde(default)]
    pub notify: bool,
}

/// Request payload for `POST /api/v1/settings/ai/test` (redacted `Debug`).
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTestRequest {
    pub provider_label: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl fmt::Debug for AiTestRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .api_key
            .as_ref()
            .filter(|k| !k.is_empty())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("AiTestRequest")
            .field("provider_label", &self.provider_label)
            .field("base_url", &self.base_url)
            .field("api_key", &redacted)
            .field("model", &self.model)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .finish()
    }
}

/// Connectivity test outcome returned by `POST /api/v1/settings/ai/test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTestResult {
    pub ok: bool,
    pub provider_label: String,
    pub model: String,
    pub latency_ms: u64,
    pub message: String,
}

/// Live preview outcome returned by `POST /api/v1/settings/sources/{id}/test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTestResult {
    pub source_id: Uuid,
    pub outcome: FetchOutcome,
    pub items_fetched: usize,
    pub sample_titles: Vec<String>,
    pub error_message: Option<String>,
}

/// Test dispatch outcome returned by `POST /api/v1/settings/channels/{id}/test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelTestResult {
    pub channel_id: Uuid,
    pub channel_type: NotificationChannelType,
    pub status: DeliveryStatus,
    pub error_message: Option<String>,
    pub sent_at: DateTime<Utc>,
}

/// Orchestration boundary invoked by REST handlers for execution and connectivity tests.
#[async_trait]
pub trait ExecutionOrchestrator: Send + Sync {
    async fn trigger_manual_run(
        &self,
        user_id: Uuid,
        req: TriggerRunRequest,
    ) -> Result<DigestRunView, CoreError>;

    async fn test_ai_config(
        &self,
        user_id: Uuid,
        req: AiTestRequest,
    ) -> Result<AiTestResult, CoreError>;

    async fn test_source(
        &self,
        user_id: Uuid,
        source_id: Uuid,
    ) -> Result<SourceTestResult, CoreError>;

    async fn test_channel(
        &self,
        user_id: Uuid,
        channel_id: Uuid,
    ) -> Result<ChannelTestResult, CoreError>;

    async fn resend_delivery(
        &self,
        user_id: Uuid,
        delivery_id: Uuid,
    ) -> Result<DeliveryLogRecord, CoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_test_request_redacts_api_key_in_debug() {
        let req = AiTestRequest {
            provider_label: Some("DeepSeek".to_owned()),
            base_url: Some("https://api.deepseek.com".to_owned()),
            api_key: Some("test-secret-api-key-value".to_owned()),
            model: Some("deepseek-chat".to_owned()),
            system_prompt: None,
            temperature: Some(0.2),
            max_tokens: Some(1024),
        };
        let rendered = format!("{req:?}");
        assert!(!rendered.contains("test-secret-api-key-value"));
        assert!(rendered.contains(SECRET_MASK_REDACTED_PLACEHOLDER));
    }
}
