//! Real-time per-user Server-Sent Events broadcast hub.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use digest_core::domain::{RunStatus, RunType, SseEventType};
use serde::Serialize;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::constants::SSE_CHANNEL_CAPACITY;
use crate::repo::{DailyDigestDetail, PendingAlertItem};
use crate::services::{SseEnvelope, SsePublisher};

/// Thread-safe per-user broadcast hub for Server-Sent Events (`SSE`).
///
/// Inactive user channels (zero active receivers) are pruned during `publish`
/// to keep memory usage bounded over long server lifetimes.
#[derive(Clone, Default)]
pub struct SseHub {
    channels: Arc<Mutex<HashMap<Uuid, broadcast::Sender<SseEnvelope>>>>,
}

impl SseHub {
    /// Creates an empty `SseHub`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of user channels currently tracked in memory.
    #[must_use]
    pub fn active_user_channels(&self) -> usize {
        let guard = self
            .channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.len()
    }

    /// Removes all channels that currently have zero active receivers.
    pub fn prune_idle_channels(&self) {
        let mut guard = self
            .channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.retain(|_, sender| sender.receiver_count() > 0);
    }
}

impl SsePublisher for SseHub {
    fn publish(&self, user_id: Uuid, event: SseEnvelope) {
        let maybe_sender = {
            let mut guard = self
                .channels
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.retain(|_, sender| sender.receiver_count() > 0);
            guard.get(&user_id).cloned()
        };

        if let Some(sender) = maybe_sender {
            if let Err(err) = sender.send(event) {
                tracing::debug!(
                    user_id = %user_id,
                    error = %err,
                    "SSE receiver dropped before delivery"
                );
            }
        }
    }

    fn subscribe(&self, user_id: Uuid) -> broadcast::Receiver<SseEnvelope> {
        let mut guard = self
            .channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(sender) = guard.get(&user_id) {
            return sender.subscribe();
        }

        let (sender, receiver) = broadcast::channel(SSE_CHANNEL_CAPACITY);
        guard.insert(user_id, sender);
        receiver
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunProgressPayload<'a> {
    run_id: Uuid,
    run_type: RunType,
    status: RunStatus,
    stage: &'a str,
    items_fetched: i32,
    items_new: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DigestReadyPayload<'a> {
    digest_id: Uuid,
    digest_date: chrono::NaiveDate,
    title: &'a str,
    item_count: i32,
    highlighted_count: i32,
    degraded_fallback: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AlertTriggeredItem<'a> {
    item_id: Uuid,
    source_id: Uuid,
    source_name: &'a str,
    title: &'a str,
    canonical_url: &'a str,
    priority: digest_core::domain::AlertPriority,
    matched_keywords: &'a [String],
    published_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AlertTriggeredPayload<'a> {
    run_id: Uuid,
    count: usize,
    items: Vec<AlertTriggeredItem<'a>>,
}

/// Builds a secret-free `run_progress` SSE envelope.
#[must_use]
pub fn run_progress_envelope(
    run_id: Uuid,
    run_type: RunType,
    status: RunStatus,
    stage: &str,
    items_fetched: i32,
    items_new: i32,
    message: Option<&str>,
) -> SseEnvelope {
    let payload = serde_json::to_value(RunProgressPayload {
        run_id,
        run_type,
        status,
        stage,
        items_fetched,
        items_new,
        message,
    })
    .unwrap_or_else(|_| serde_json::json!({ "runId": run_id }));

    SseEnvelope {
        event_type: SseEventType::RunProgress,
        payload,
    }
}

/// Builds a secret-free `digest_ready` SSE envelope.
#[must_use]
pub fn digest_ready_envelope(digest: &DailyDigestDetail) -> SseEnvelope {
    let payload = serde_json::to_value(DigestReadyPayload {
        digest_id: digest.id,
        digest_date: digest.digest_date,
        title: &digest.title,
        item_count: digest.item_count,
        highlighted_count: digest.highlighted_count,
        degraded_fallback: digest.degraded_fallback,
    })
    .unwrap_or_else(|_| serde_json::json!({ "digestId": digest.id }));

    SseEnvelope {
        event_type: SseEventType::DigestReady,
        payload,
    }
}

/// Builds a secret-free `alert_triggered` SSE envelope.
#[must_use]
pub fn alert_triggered_envelope(run_id: Uuid, alerts: &[PendingAlertItem]) -> SseEnvelope {
    let items = alerts
        .iter()
        .map(|item| AlertTriggeredItem {
            item_id: item.item_id,
            source_id: item.source_id,
            source_name: &item.source_name,
            title: &item.title,
            canonical_url: &item.canonical_url,
            priority: item.priority,
            matched_keywords: &item.matched_keywords,
            published_at: item.published_at,
        })
        .collect();

    let payload = serde_json::to_value(AlertTriggeredPayload {
        run_id,
        count: alerts.len(),
        items,
    })
    .unwrap_or_else(|_| serde_json::json!({ "runId": run_id, "count": alerts.len() }));

    SseEnvelope {
        event_type: SseEventType::AlertTriggered,
        payload,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, Utc};
    use digest_core::domain::AlertPriority;

    use super::*;

    #[tokio::test]
    async fn sse_hub_isolates_users_and_prunes_dead_channels() {
        let hub = SseHub::new();
        let user_a = Uuid::new_v4();
        let user_b = Uuid::new_v4();

        let mut rx_a1 = hub.subscribe(user_a);
        let mut rx_a2 = hub.subscribe(user_a);
        let rx_b = hub.subscribe(user_b);
        assert_eq!(hub.active_user_channels(), 2);

        // Drop user B's only receiver; publishing to user A prunes user B's idle channel.
        drop(rx_b);

        let event = run_progress_envelope(
            Uuid::new_v4(),
            RunType::ManualDaily,
            RunStatus::Running,
            "collecting",
            3,
            2,
            Some("fetching sources"),
        );
        hub.publish(user_a, event.clone());

        assert_eq!(hub.active_user_channels(), 1);
        assert_eq!(rx_a1.recv().await.unwrap(), event);
        assert_eq!(rx_a2.recv().await.unwrap(), event);

        // Drop remaining receivers and publish again -> all channels pruned.
        drop(rx_a1);
        drop(rx_a2);
        hub.publish(user_a, event);
        assert_eq!(hub.active_user_channels(), 0);
    }

    #[test]
    fn sse_envelope_helpers_emit_camel_case_without_secrets() {
        let digest = DailyDigestDetail {
            id: Uuid::new_v4(),
            digest_date: NaiveDate::from_ymd_opt(2026, 11, 26).unwrap(),
            input_fingerprint: "fp-1".to_owned(),
            title: "Daily Digest".to_owned(),
            overview_markdown: "Overview".to_owned(),
            overview_html: "<p>Overview</p>".to_owned(),
            sections: Vec::new(),
            item_count: 5,
            highlighted_count: 1,
            prompt_tokens: 10,
            completion_tokens: 20,
            model_used: "deepseek-chat".to_owned(),
            degraded_fallback: false,
            created_at: Utc::now(),
        };
        let ready = digest_ready_envelope(&digest);
        assert_eq!(ready.event_type, SseEventType::DigestReady);
        assert_eq!(ready.payload["itemCount"], 5);
        assert_eq!(ready.payload["highlightedCount"], 1);
        assert!(ready.payload.get("apiKey").is_none());

        let alert = alert_triggered_envelope(
            Uuid::new_v4(),
            &[PendingAlertItem {
                item_id: Uuid::new_v4(),
                source_id: Uuid::new_v4(),
                source_name: "Official Blog".to_owned(),
                category: "Official".to_owned(),
                canonical_url: "https://example.com/post".to_owned(),
                title: "Gemini Update".to_owned(),
                author: None,
                summary: None,
                published_at: None,
                matched_keywords: vec!["Gemini".to_owned()],
                priority: AlertPriority::Urgent,
                importance_score: 90,
            }],
        );
        assert_eq!(alert.event_type, SseEventType::AlertTriggered);
        assert_eq!(alert.payload["count"], 1);
        assert_eq!(alert.payload["items"][0]["matchedKeywords"][0], "Gemini");
    }
}
