//! High-frequency alert scanner evaluating `user_schedules` with `alert_scan_enabled`,
//! refreshing due subscribed sources, matching urgent watchlist rules, emitting SSE
//! `alert_triggered` events, dispatching `trigger_on_alert` channels, and recording
//! `digest_runs` (`RunType::AlertScan`) + `delivery_logs`.

use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use digest_core::domain::{
    AlertPriority, DeliveryStatus, FetchOutcome, ItemMatchResult, NotificationChannelType,
    ProcessedItem, RenderInput, RunStatus, RunType, WatchlistRuleInput,
};
use digest_core::error::CoreError;
use digest_core::pipeline::priority_rank;
use digest_core::renderer::RendererRegistry;
use digest_core::traits::{DigestRenderer, RenderedNotification, SummarizedDigest};
use futures::stream::{self, StreamExt};
use uuid::Uuid;

use crate::repo::{
    CanonicalItemRecord, CreateRunCmd, FinishRunCmd, PendingAlertItem, UserScheduleRecord,
    UserSourceRecord, WatchlistRuleRecord,
};
use crate::scheduler::composer::{
    dispatch_selected_channels, match_and_score_batches, ComposerDeps,
};
use crate::scheduler::poller::{
    canonical_source_from_user_source, poll_canonical_source_once_with_platform_x,
    SourcePollOutcome,
};
use crate::sse_hub::alert_triggered_envelope;

const ALERT_MODEL_LABEL: &str = "alert-scanner";

/// Summary of an alert scanner tick across due users.
#[derive(Debug, Clone, Default)]
pub struct AlertScanTickReport {
    pub users_scanned: usize,
    pub alerts_triggered: usize,
    pub deliveries_attempted: usize,
}

/// Runs one high-frequency alert scanner tick using `Utc::now()`.
pub async fn run_alert_scan_tick(deps: &ComposerDeps) -> Result<AlertScanTickReport, CoreError> {
    run_alert_scan_tick_at(deps, Utc::now()).await
}

/// Runs one high-frequency alert scanner tick evaluated at `now_utc`.
pub async fn run_alert_scan_tick_at(
    deps: &ComposerDeps,
    now_utc: DateTime<Utc>,
) -> Result<AlertScanTickReport, CoreError> {
    let due_schedules = deps
        .repos
        .settings
        .list_due_alert_schedules(now_utc)
        .await?;

    let mut report = AlertScanTickReport {
        users_scanned: due_schedules.len(),
        ..AlertScanTickReport::default()
    };

    for schedule in due_schedules {
        match scan_user_alerts(deps, &schedule, now_utc).await {
            Ok((alert_count, delivery_count)) => {
                report.alerts_triggered = report.alerts_triggered.saturating_add(alert_count);
                report.deliveries_attempted =
                    report.deliveries_attempted.saturating_add(delivery_count);
            }
            Err(err) => {
                tracing::error!(
                    user_id = %schedule.user_id,
                    error = %err,
                    "alert scan failed for user"
                );
            }
        }
    }

    Ok(report)
}

async fn scan_user_alerts(
    deps: &ComposerDeps,
    schedule: &UserScheduleRecord,
    now_utc: DateTime<Utc>,
) -> Result<(usize, usize), CoreError> {
    let user_id = schedule.user_id;
    let source_views = deps.repos.sources.list_user_sources(user_id).await?;
    let mut enabled_sources = Vec::new();
    for view in source_views.into_iter().filter(|v| v.enabled) {
        if let Some(rec) = deps.repos.sources.get_user_source(user_id, view.id).await? {
            if rec.enabled {
                enabled_sources.push(rec);
            }
        }
    }

    let due_sources: Vec<UserSourceRecord> = enabled_sources
        .iter()
        .filter(|src| src.next_poll_at.is_none_or(|due| due <= now_utc))
        .cloned()
        .collect();

    let concurrency = deps
        .config
        .core
        .resources
        .global_fetch_concurrency
        .get()
        .max(1);

    let poll_outcomes: Vec<SourcePollOutcome> = stream::iter(due_sources)
        .map(|user_src| {
            let repos = deps.repos.clone();
            let cache = Arc::clone(&deps.cache);
            let cipher = Arc::clone(&deps.cipher);
            let collectors = Arc::clone(&deps.collectors);
            let pipeline = Arc::clone(&deps.pipeline);
            let collector_tuning = deps.config.core.collector.clone();
            let limits = deps.config.core.resources.clone();
            let env_bridge = deps.config.platform_x_bridge_base_url.clone();
            async move {
                let canonical = canonical_source_from_user_source(&user_src);
                match poll_canonical_source_once_with_platform_x(
                    &repos,
                    &cache,
                    &cipher,
                    &collectors,
                    &pipeline,
                    &collector_tuning,
                    &limits,
                    &canonical,
                    false,
                    env_bridge.as_deref(),
                )
                .await
                {
                    Ok(out) => out,
                    Err(err) => SourcePollOutcome {
                        source_id: canonical.id,
                        outcome: FetchOutcome::Failed,
                        items_fetched: 0,
                        stored_items: Vec::new(),
                        error_message: Some(err.to_string()),
                    },
                }
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let items_fetched_i32 =
        i32::try_from(poll_outcomes.iter().map(|o| o.items_fetched).sum::<usize>())
            .unwrap_or(i32::MAX);
    let any_fetch_failed = poll_outcomes
        .iter()
        .any(|o| o.outcome == FetchOutcome::Failed);

    // Evaluate watchlist rules over recent items from enabled sources.
    let watchlist_records = deps.repos.settings.list_watchlist_rules(user_id).await?;
    let watchlist_inputs: Vec<WatchlistRuleInput> = watchlist_records
        .iter()
        .filter(|r| r.enabled)
        .map(WatchlistRuleRecord::to_core_input)
        .collect();

    if !watchlist_inputs.is_empty() && !enabled_sources.is_empty() {
        let lookback_hours = i64::from(schedule.alert_interval_minutes.max(60) / 60).max(2);
        let since = now_utc - ChronoDuration::hours(lookback_hours.max(24));
        let max_items = deps.config.core.llm.max_items_per_digest.max(1);
        let mut batches: Vec<(UserSourceRecord, Vec<CanonicalItemRecord>)> =
            Vec::with_capacity(enabled_sources.len());
        for src in &enabled_sources {
            let items = deps
                .repos
                .sources
                .list_recent_source_items(src.canonical_source_id, since, max_items)
                .await?;
            batches.push((src.clone(), items));
        }

        let (matches, _) = tokio::task::spawn_blocking(move || {
            match_and_score_batches(&batches, &watchlist_inputs, now_utc)
        })
        .await
        .map_err(|err| CoreError::Internal(format!("alert match task failed: {err}")))?;

        if !matches.is_empty() {
            deps.repos
                .sources
                .upsert_user_item_matches(user_id, &matches)
                .await?;
        }
    }

    let pending_alerts = deps
        .repos
        .sources
        .list_pending_urgent_alerts(user_id)
        .await?;

    if pending_alerts.is_empty() {
        deps.repos
            .settings
            .mark_alert_scan_at(user_id, now_utc)
            .await?;
        return Ok((0, 0));
    }

    let run = deps
        .repos
        .runs
        .create_run(CreateRunCmd {
            user_id,
            run_type: RunType::AlertScan,
            started_at: now_utc,
        })
        .await?;

    deps.sse
        .publish(user_id, alert_triggered_envelope(run.id, &pending_alerts));

    let rendered =
        render_alert_notification_blocking(Arc::clone(&deps.renderer), now_utc, &pending_alerts)
            .await?;

    let channels = deps.repos.settings.list_channels(user_id).await?;
    let selected_channels = channels
        .into_iter()
        .filter(|ch| {
            ch.enabled
                && ch.trigger_on_alert
                && (ch.channel_type != NotificationChannelType::Smtp || schedule.email_enabled)
        })
        .collect();

    let deliveries = dispatch_selected_channels(
        &deps.repos,
        Arc::clone(&deps.cipher),
        Arc::clone(&deps.notifiers),
        user_id,
        run.id,
        None,
        selected_channels,
        &rendered,
        concurrency,
    )
    .await;

    let alert_ids: Vec<Uuid> = pending_alerts.iter().map(|a| a.item_id).collect();
    deps.repos
        .sources
        .mark_items_alerted(user_id, &alert_ids, Utc::now())
        .await?;

    let any_delivery_failed = deliveries
        .iter()
        .any(|d| d.status == DeliveryStatus::Failed);
    let final_status = if any_fetch_failed || any_delivery_failed {
        RunStatus::Partial
    } else {
        RunStatus::Succeeded
    };
    let error_summary = match (any_fetch_failed, any_delivery_failed) {
        (true, true) => Some("one or more sources and alert deliveries failed".to_owned()),
        (true, false) => Some("one or more sources failed during alert scan".to_owned()),
        (false, true) => Some("one or more alert deliveries failed".to_owned()),
        (false, false) => None,
    };

    deps.repos
        .runs
        .finish_run(
            run.id,
            FinishRunCmd {
                status: final_status,
                finished_at: Utc::now(),
                items_fetched: items_fetched_i32,
                items_new: i32::try_from(pending_alerts.len()).unwrap_or(i32::MAX),
                prompt_tokens: 0,
                completion_tokens: 0,
                error_summary,
            },
        )
        .await?;

    deps.repos
        .settings
        .mark_alert_scan_at(user_id, now_utc)
        .await?;

    Ok((pending_alerts.len(), deliveries.len()))
}

async fn render_alert_notification_blocking(
    renderer: Arc<RendererRegistry>,
    now_utc: DateTime<Utc>,
    alerts: &[PendingAlertItem],
) -> Result<RenderedNotification, CoreError> {
    let highest_priority = alerts
        .iter()
        .map(|a| a.priority)
        .max_by_key(|p| priority_rank(*p))
        .unwrap_or(AlertPriority::Urgent);

    let processed: Vec<ProcessedItem> = alerts
        .iter()
        .map(|a| ProcessedItem {
            external_id: None,
            canonical_url: a.canonical_url.clone(),
            content_hash: a.item_id.to_string(),
            title: a.title.clone(),
            author: a.author.clone(),
            summary: a.summary.clone(),
            published_at: a.published_at,
            source_id: Some(a.source_id),
            source_name: Some(a.source_name.clone()),
            category: Some(a.category.clone()),
            match_result: ItemMatchResult {
                matched_keywords: a.matched_keywords.clone(),
                priority: a.priority,
                importance_score: a.importance_score,
                notify_immediately: true,
            },
        })
        .collect();

    let mut overview_lines = Vec::with_capacity(alerts.len() + 2);
    overview_lines.push(format!(
        "{} urgent watchlist match(es) detected:",
        alerts.len()
    ));
    overview_lines.push(String::new());
    for item in alerts {
        let keywords = if item.matched_keywords.is_empty() {
            String::new()
        } else {
            format!(" [{}]", item.matched_keywords.join(", "))
        };
        overview_lines.push(format!(
            "- **{}**{} — [{}]({})",
            item.source_name, keywords, item.title, item.canonical_url
        ));
    }

    let render_input = RenderInput {
        digest_date: now_utc.date_naive(),
        digest: SummarizedDigest {
            title: format!("Gemini Urgent Alert ({} match(es))", alerts.len()),
            overview_markdown: overview_lines.join("\n"),
            prompt_tokens: 0,
            completion_tokens: 0,
            model_used: ALERT_MODEL_LABEL.to_owned(),
            degraded_fallback: false,
        },
        sections: Vec::new(),
        highlighted_items: processed.clone(),
        all_items: processed,
        priority: highest_priority,
        locale: digest_core::domain::default_digest_locale(),
    };

    tokio::task::spawn_blocking(move || renderer.render_structured(&render_input))
        .await
        .map_err(|err| CoreError::Internal(format!("alert render task failed: {err}")))?
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use digest_core::cache::{CacheCoordinator, InMemoryCacheCoordinator};
    use digest_core::collectors::{CollectorContext, CollectorRegistry};
    use digest_core::config::{CollectorTuning, NotifierTuning};
    use digest_core::crypto::EnvelopeCipher;
    use digest_core::domain::{SourceScope, SourceType, SseEventType};
    use digest_core::http::shared_http_client;
    use digest_core::llm::{IdempotentLlmRunner, OpenAiCompatProvider};
    use digest_core::notifiers::NotifierRegistry;
    use digest_core::pipeline::ItemProcessingPipeline;
    use digest_core::ssrf::SsrfGuard;
    use digest_core::traits::StructuredLlmProvider;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;
    use crate::config::ServerConfig;
    use crate::repo::{
        encode_source_config, CreateChannelRecord, PageRequest, SubscribeSourceCmd,
        UpdateScheduleCmd, UpsertWatchlistRuleCmd,
    };
    use crate::scheduler::test_support::MockRepos;
    use crate::sse_hub::SseHub;

    #[tokio::test]
    async fn alert_scanner_detects_urgent_match_emits_sse_and_marks_alerted() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/alerts.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Official</title><item><guid>u1</guid><title>Gemini 2.5 Flash Live</title><link>https://example.com/flash</link><description>Now available.</description></item></channel></rss>"#,
            ))
            .mount(&server)
            .await;

        let webhook_hits = Arc::new(AtomicUsize::new(0));
        let webhook_hits_clone = Arc::clone(&webhook_hits);
        Mock::given(method("POST"))
            .and(path("/alert-hook"))
            .respond_with(move |_req: &Request| {
                webhook_hits_clone.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
            })
            .mount(&server)
            .await;

        let mut cfg = ServerConfig::default();
        cfg.core.collector = CollectorTuning {
            max_retry_attempts: NonZeroUsize::new(1).unwrap(),
            initial_backoff: Duration::from_millis(5),
            ..CollectorTuning::default()
        };
        cfg.core.ssrf_allowlist_hosts = vec!["127.0.0.1".to_owned(), "localhost".to_owned()];

        let client = shared_http_client(None, &cfg.core.collector).unwrap();
        let ssrf = SsrfGuard::from_config(&cfg.core);
        let cache: Arc<dyn CacheCoordinator> = Arc::new(InMemoryCacheCoordinator::new());
        let collectors = Arc::new(CollectorRegistry::new(CollectorContext::new(
            client.clone(),
            ssrf.clone(),
            cfg.core.collector.clone(),
            cfg.core.resources.clone(),
        )));
        let provider: Arc<dyn StructuredLlmProvider> =
            Arc::new(OpenAiCompatProvider::new(client.clone(), ssrf.clone()));
        let llm_runner = Arc::new(IdempotentLlmRunner::new(Arc::clone(&cache), provider));
        let notifiers = Arc::new(NotifierRegistry::with_defaults(
            client,
            ssrf,
            NotifierTuning::default(),
        ));
        let mock = MockRepos::new();
        let deps = ComposerDeps {
            config: Arc::new(cfg),
            repos: mock.repositories(),
            cipher: Arc::new(EnvelopeCipher::from_bytes([0x33u8; 32])),
            cache,
            collectors,
            pipeline: Arc::new(ItemProcessingPipeline::new()),
            llm_runner,
            renderer: Arc::new(RendererRegistry::new()),
            notifiers,
            sse: Arc::new(SseHub::new()),
        };

        let user_id = Uuid::new_v4();
        let mut sse_rx = deps.sse.subscribe(user_id);

        deps.repos
            .settings
            .upsert_schedule(
                user_id,
                UpdateScheduleCmd {
                    timezone: "UTC".to_owned(),
                    daily_time: "08:30".to_owned(),
                    email_enabled: true,
                    send_when_empty: false,
                    alert_scan_enabled: true,
                    alert_interval_minutes: 15,
                },
            )
            .await
            .unwrap();

        let src_enc =
            encode_source_config(&deps.cipher, &format!("{}/alerts.xml", server.uri()), None)
                .unwrap();
        deps.repos
            .sources
            .create_or_subscribe(
                user_id,
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "alert-rss".to_owned(),
                    custom_name: "Official".to_owned(),
                    default_name: "Official".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: format!("{}/alerts.xml", server.uri()),
                    config_json_enc: Some(src_enc),
                    enabled: true,
                },
            )
            .await
            .unwrap();

        deps.repos
            .settings
            .create_watchlist_rule(
                user_id,
                UpsertWatchlistRuleCmd {
                    name: "Flash".to_owned(),
                    keywords: vec!["Gemini 2.5".to_owned()],
                    exclude_keywords: Vec::new(),
                    priority: AlertPriority::Urgent,
                    notify_immediately: true,
                    enabled: true,
                },
            )
            .await
            .unwrap();

        let hook_enc = deps
            .cipher
            .encrypt(
                &serde_json::json!({ "url": format!("{}/alert-hook", server.uri()) }).to_string(),
            )
            .unwrap();
        deps.repos
            .settings
            .create_channel(
                user_id,
                CreateChannelRecord {
                    channel_type: NotificationChannelType::Webhook,
                    name: "Alert Hook".to_owned(),
                    enabled: true,
                    config_enc: hook_enc,
                    config_hint: "https://example.com".to_owned(),
                    trigger_on_daily: false,
                    trigger_on_alert: true,
                },
            )
            .await
            .unwrap();

        let now = Utc::now();
        let report = run_alert_scan_tick_at(&deps, now).await.unwrap();
        assert_eq!(report.users_scanned, 1);
        assert_eq!(report.alerts_triggered, 1);
        assert_eq!(report.deliveries_attempted, 1);
        assert_eq!(webhook_hits.load(Ordering::SeqCst), 1);

        let sse_event = sse_rx.try_recv().unwrap();
        assert_eq!(sse_event.event_type, SseEventType::AlertTriggered);
        assert_eq!(sse_event.payload["count"], 1);

        // Second tick immediately after -> interval not elapsed -> 0 users scanned.
        let report2 = run_alert_scan_tick_at(&deps, now + ChronoDuration::minutes(1))
            .await
            .unwrap();
        assert_eq!(report2.users_scanned, 0);

        // Even after interval elapses, item was marked alerted -> 0 duplicate alerts.
        let report3 = run_alert_scan_tick_at(&deps, now + ChronoDuration::minutes(20))
            .await
            .unwrap();
        assert_eq!(report3.users_scanned, 1);
        assert_eq!(report3.alerts_triggered, 0);
        assert_eq!(webhook_hits.load(Ordering::SeqCst), 1);

        let runs = deps
            .repos
            .runs
            .list_runs_with_deliveries(user_id, PageRequest::default())
            .await
            .unwrap();
        assert_eq!(runs.items.len(), 1);
        assert_eq!(runs.items[0].run_type, RunType::AlertScan);
        assert_eq!(runs.items[0].deliveries.len(), 1);
        assert_eq!(runs.items[0].deliveries[0].status, DeliveryStatus::Sent);
    }
}
