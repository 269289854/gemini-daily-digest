//! Authenticated Server-Sent Events stream (`GET /api/v1/events/stream`).

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use chrono::Utc;
use digest_core::domain::SseEventType;
use futures::stream::{self, Stream, StreamExt};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;

use crate::auth::AuthenticatedUser;
use crate::constants::SSE_HEARTBEAT_INTERVAL_SECS;
use crate::state::AppState;

/// `GET /api/v1/events/stream` — authenticated real-time SSE stream per user.
///
/// Emits `run_progress`, `digest_ready`, `alert_triggered` events from `SsePublisher`
/// plus periodic `heartbeat` events on `SSE_HEARTBEAT_INTERVAL_SECS`.
pub async fn stream_events(
    State(state): State<AppState>,
    AuthenticatedUser(user): AuthenticatedUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.sse.subscribe(user.id);
    let interval = Duration::from_secs(SSE_HEARTBEAT_INTERVAL_SECS);

    let events_stream = BroadcastStream::new(rx).filter_map(|item| async move {
        match item {
            Ok(envelope) => {
                let data =
                    serde_json::to_string(&envelope.payload).unwrap_or_else(|_| "{}".to_owned());
                Some(Ok::<Event, Infallible>(
                    Event::default()
                        .event(envelope.event_type.as_str())
                        .data(data),
                ))
            }
            Err(BroadcastStreamRecvError::Lagged(skipped)) => {
                tracing::warn!(skipped, "SSE subscriber lagged behind broadcast channel");
                None
            }
        }
    });

    let start = tokio::time::Instant::now() + interval;
    let heartbeat_stream = stream::unfold(
        tokio::time::interval_at(start, interval),
        |mut ticker| async move {
            ticker.tick().await;
            let payload = serde_json::json!({
                "timestamp": Utc::now(),
            })
            .to_string();
            let ev = Ok::<Event, Infallible>(
                Event::default()
                    .event(SseEventType::Heartbeat.as_str())
                    .data(payload),
            );
            Some((ev, ticker))
        },
    );

    let combined = stream::select(events_stream, heartbeat_stream);
    Sse::new(combined).keep_alive(KeepAlive::new().interval(interval))
}
