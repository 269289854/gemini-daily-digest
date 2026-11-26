#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, NaiveDate, Utc};
use digest_core::cache::{CacheCoordinator, InMemoryCacheCoordinator};
use digest_core::collectors::{CollectorContext, CollectorRegistry};
use digest_core::config::{AppConfig, CollectorTuning, LlmTuning, NotifierTuning};
use digest_core::constants::{CRYPTO_KEY_BYTE_LEN, DEFAULT_SOURCE_FRESHNESS_TTL_SECS};
use digest_core::crypto::EnvelopeCipher;
use digest_core::domain::{
    AlertPriority, FetchOutcome, NotificationChannelType, RunType, SourceScope, SourceType,
    SseEventType,
};
use digest_core::http::shared_http_client;
use digest_core::llm::{IdempotentLlmRunner, OpenAiCompatProvider};
use digest_core::notifiers::NotifierRegistry;
use digest_core::pipeline::ItemProcessingPipeline;
use digest_core::renderer::RendererRegistry;
use digest_core::ssrf::SsrfGuard;
use digest_core::traits::StructuredLlmProvider;
use digest_server::config::ServerConfig;
use digest_server::repo::in_memory::InMemoryRepoStore;
use digest_server::repo::{
    encode_source_config, CreateChannelRecord, NewCanonicalItem, PageRequest, Repositories,
    SubscribeSourceCmd, UpdateScheduleCmd, UpdateSourceCmd, UpsertAiConfigRecord, UpsertUserCmd,
    UpsertWatchlistRuleCmd,
};
use digest_server::scheduler::alert_scanner::run_alert_scan_tick_at;
use digest_server::scheduler::poller::{poll_canonical_source_once, run_poller_tick};
use digest_server::scheduler::retention::run_retention_once;
use digest_server::scheduler::{ComposerDeps, ServerExecutionOrchestrator};
use digest_server::services::SsePublisher;
use digest_server::sse_hub::SseHub;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn build_scheduler_harness() -> (Repositories, ComposerDeps, ServerExecutionOrchestrator) {
    let mut core = AppConfig {
        collector: CollectorTuning {
            max_retry_attempts: std::num::NonZeroUsize::new(1).unwrap(),
            initial_backoff: Duration::from_millis(5),
            ..CollectorTuning::default()
        },
        llm: LlmTuning {
            request_timeout: Duration::from_secs(5),
            ..LlmTuning::default()
        },
        ssrf_allowlist_hosts: vec!["127.0.0.1".to_owned(), "localhost".to_owned()],
        ..AppConfig::default()
    };
    core.resources.source_freshness_ttl = Duration::from_secs(DEFAULT_SOURCE_FRESHNESS_TTL_SECS);

    let config = Arc::new(ServerConfig {
        core,
        ..ServerConfig::default()
    });
    let cipher = Arc::new(EnvelopeCipher::from_bytes([0x77u8; CRYPTO_KEY_BYTE_LEN]));
    let ssrf = SsrfGuard::from_config(&config.core);
    let http = shared_http_client(None, &config.core.collector).unwrap();
    let cache: Arc<dyn CacheCoordinator> = Arc::new(InMemoryCacheCoordinator::new());
    let repos = InMemoryRepoStore::repositories_with_cipher(Arc::clone(&cipher));

    let collectors = Arc::new(CollectorRegistry::new(CollectorContext::new(
        http.clone(),
        ssrf.clone(),
        config.core.collector.clone(),
        config.core.resources.clone(),
    )));
    let pipeline = Arc::new(ItemProcessingPipeline::default());
    let llm_provider: Arc<dyn StructuredLlmProvider> = Arc::new(OpenAiCompatProvider::with_tuning(
        http.clone(),
        ssrf.clone(),
        config.core.llm.clone(),
        config.core.resources.max_feed_bytes.get(),
    ));
    let llm_runner = Arc::new(IdempotentLlmRunner::new(
        Arc::clone(&cache),
        Arc::clone(&llm_provider),
    ));
    let renderer = Arc::new(RendererRegistry::new());
    let notifiers = Arc::new(NotifierRegistry::with_defaults(
        http,
        ssrf.clone(),
        NotifierTuning::default(),
    ));
    let sse: Arc<dyn SsePublisher> = Arc::new(SseHub::new());

    let deps = ComposerDeps {
        config,
        repos: repos.clone(),
        cipher,
        cache,
        collectors,
        pipeline,
        llm_runner,
        renderer,
        notifiers,
        sse,
    };
    let orchestrator = ServerExecutionOrchestrator::new(deps.clone(), ssrf, llm_provider);
    (repos, deps, orchestrator)
}

async fn create_test_user(repos: &Repositories, sub: &str) -> Uuid {
    repos
        .users
        .upsert_oidc_user(UpsertUserCmd {
            oidc_sub: sub.to_owned(),
            email: Some(format!("{sub}@example.com")),
            display_name: Some(sub.to_owned()),
            role: "member".to_owned(),
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn zero_subscriber_sleep_makes_zero_http_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sleep.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<rss/>"))
        .expect(0)
        .mount(&server)
        .await;

    let (repos, deps, _) = build_scheduler_harness();
    let user_id = create_test_user(&repos, "sleep-user").await;

    let feed_url = format!("{}/sleep.xml", server.uri());
    let enc = encode_source_config(&deps.cipher, &feed_url, None).unwrap();
    let src = repos
        .sources
        .create_or_subscribe(
            user_id,
            SubscribeSourceCmd {
                source_type: SourceType::Rss,
                scope: SourceScope::SharedPublic,
                canonical_key: "sleep-rss".to_owned(),
                custom_name: "Sleep RSS".to_owned(),
                default_name: "Sleep RSS".to_owned(),
                category: "Official".to_owned(),
                handle_or_url: feed_url,
                config_json_enc: Some(enc),
                enabled: true,
            },
        )
        .await
        .unwrap();

    // Disable the only subscription -> canonical source must sleep with 0 network calls.
    repos
        .sources
        .update_user_subscription(
            user_id,
            src.id,
            UpdateSourceCmd {
                enabled: Some(false),
                ..UpdateSourceCmd::default()
            },
        )
        .await
        .unwrap();

    let report = run_poller_tick(
        &repos,
        &deps.cache,
        &deps.cipher,
        &deps.collectors,
        &deps.pipeline,
        &deps.config.core.collector,
        &deps.config.core.resources,
    )
    .await
    .unwrap();
    assert_eq!(report.sources_polled, 0);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn shared_public_single_fetch_across_two_users_with_singleflight_and_freshness_ttl() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/shared-feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Shared</title><item><guid>sf-1</guid><title>Shared Canonical Post</title><link>https://example.com/sf/1</link><description>Body text</description></item></channel></rss>"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let (repos, deps, _) = build_scheduler_harness();
    let user_a = create_test_user(&repos, "shared-a").await;
    let user_b = create_test_user(&repos, "shared-b").await;

    let feed_url = format!("{}/shared-feed.xml", server.uri());
    let enc_a = encode_source_config(&deps.cipher, &feed_url, None).unwrap();
    let enc_b = encode_source_config(&deps.cipher, &feed_url, None).unwrap();

    let src_a = repos
        .sources
        .create_or_subscribe(
            user_a,
            SubscribeSourceCmd {
                source_type: SourceType::Rss,
                scope: SourceScope::SharedPublic,
                canonical_key: "shared-canonical-feed".to_owned(),
                custom_name: "Feed A".to_owned(),
                default_name: "Shared Feed".to_owned(),
                category: "Official".to_owned(),
                handle_or_url: feed_url.clone(),
                config_json_enc: Some(enc_a),
                enabled: true,
            },
        )
        .await
        .unwrap();
    let src_b = repos
        .sources
        .create_or_subscribe(
            user_b,
            SubscribeSourceCmd {
                source_type: SourceType::Rss,
                scope: SourceScope::SharedPublic,
                canonical_key: "shared-canonical-feed".to_owned(),
                custom_name: "Feed B".to_owned(),
                default_name: "Shared Feed".to_owned(),
                category: "Community".to_owned(),
                handle_or_url: feed_url,
                config_json_enc: Some(enc_b),
                enabled: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(src_a.id, src_b.id);

    // First poller tick fetches once across both subscribers and marks SOURCE_FRESHNESS_TTL_SECS fresh.
    let tick1 = run_poller_tick(
        &repos,
        &deps.cache,
        &deps.cipher,
        &deps.collectors,
        &deps.pipeline,
        &deps.config.core.collector,
        &deps.config.core.resources,
    )
    .await
    .unwrap();
    assert_eq!(tick1.sources_polled, 1);
    assert_eq!(tick1.updated, 1);
    assert_eq!(tick1.items_fetched, 1);

    // Second poller tick finds 0 due sources because next_poll_at was pushed out by SOURCE_FRESHNESS_TTL_SECS.
    let tick2 = run_poller_tick(
        &repos,
        &deps.cache,
        &deps.cipher,
        &deps.collectors,
        &deps.pipeline,
        &deps.config.core.collector,
        &deps.config.core.resources,
    )
    .await
    .unwrap();
    assert_eq!(tick2.sources_polled, 0);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    // Direct poll (e.g., Layer 2 composer for User B) also hits CacheCoordinator freshness TTL -> SkippedFresh.
    let shared_rec = digest_server::scheduler::poller::canonical_source_from_user_source(
        &repos
            .sources
            .get_user_source(user_b, src_b.id)
            .await
            .unwrap()
            .unwrap(),
    );
    let direct_fresh = poll_canonical_source_once(
        &repos,
        &deps.cache,
        &deps.cipher,
        &deps.collectors,
        &deps.pipeline,
        &deps.config.core.collector,
        &deps.config.core.resources,
        &shared_rec,
        false,
    )
    .await
    .unwrap();
    assert_eq!(direct_fresh.outcome, FetchOutcome::SkippedFresh);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    // Verify both users see the single canonical item.
    let items_a = repos
        .sources
        .list_source_items(user_a, src_a.id, PageRequest::default())
        .await
        .unwrap();
    let items_b = repos
        .sources
        .list_source_items(user_b, src_b.id, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(items_a.total, 1);
    assert_eq!(items_b.total, 1);
    assert_eq!(items_a.items[0].id, items_b.items[0].id);

    // Singleflight lock contention skips concurrent duplicate fetch when lock is held.
    let lock_test_url = format!("{}/locked.xml", server.uri());
    Mock::given(method("GET"))
        .and(path("/locked.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<rss/>"))
        .expect(0)
        .mount(&server)
        .await;
    let enc_lock = encode_source_config(&deps.cipher, &lock_test_url, None).unwrap();
    let locked_src = repos
        .sources
        .create_or_subscribe(
            user_a,
            SubscribeSourceCmd {
                source_type: SourceType::Rss,
                scope: SourceScope::SharedPublic,
                canonical_key: "locked-feed".to_owned(),
                custom_name: "Locked".to_owned(),
                default_name: "Locked".to_owned(),
                category: "Official".to_owned(),
                handle_or_url: lock_test_url,
                config_json_enc: Some(enc_lock),
                enabled: true,
            },
        )
        .await
        .unwrap();

    let lock_key = format!("source:{}", locked_src.id);
    assert!(deps
        .cache
        .try_acquire_lock(&lock_key, Duration::from_secs(30))
        .await
        .unwrap());
    let pollable = repos
        .sources
        .list_pollable_sources(None, Utc::now(), 10)
        .await
        .unwrap();
    let locked_rec = pollable
        .into_iter()
        .find(|s| s.id == locked_src.id)
        .unwrap();
    let contested = poll_canonical_source_once(
        &repos,
        &deps.cache,
        &deps.cipher,
        &deps.collectors,
        &deps.pipeline,
        &deps.config.core.collector,
        &deps.config.core.resources,
        &locked_rec,
        false,
    )
    .await
    .unwrap();
    assert_eq!(contested.outcome, FetchOutcome::SkippedFresh);
    deps.cache.release_lock(&lock_key).await.unwrap();
}

#[tokio::test]
async fn input_fingerprint_idempotency_avoids_duplicate_llm_calls_and_tokens() {
    let rss_server = MockServer::start().await;
    let llm_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/idem.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Idem</title><item><guid>id-1</guid><title>Stable Release Note</title><link>https://example.com/idem/1</link><description>Unchanged item content.</description></item></channel></rss>"#,
        ))
        .expect(1)
        .mount(&rss_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "deepseek-chat",
            "choices": [{
                "message": {
                    "content": "{\"summaryMarkdown\":\"Concise summary.\",\"keyPoints\":[\"Stable release\"]}"
                }
            }],
            "usage": {
                "prompt_tokens": 55,
                "completion_tokens": 22
            }
        })))
        .mount(&llm_server)
        .await;

    let (repos, deps, orchestrator) = build_scheduler_harness();
    let user_id = create_test_user(&repos, "idem-user").await;

    let api_key_enc = deps.cipher.encrypt("test-idem-ai-key").unwrap();
    repos
        .settings
        .upsert_ai_config(
            user_id,
            UpsertAiConfigRecord {
                provider_label: "MockLLM".to_owned(),
                base_url: llm_server.uri(),
                api_key_enc: Some(api_key_enc),
                api_key_hint: Some("test****-key".to_owned()),
                model: "deepseek-chat".to_owned(),
                system_prompt: "Summarize.".to_owned(),
                temperature: 0.2,
                max_tokens: 512,
            },
        )
        .await
        .unwrap();

    let feed_url = format!("{}/idem.xml", rss_server.uri());
    let enc = encode_source_config(&deps.cipher, &feed_url, None).unwrap();
    repos
        .sources
        .create_or_subscribe(
            user_id,
            SubscribeSourceCmd {
                source_type: SourceType::Rss,
                scope: SourceScope::SharedPublic,
                canonical_key: "idem-rss".to_owned(),
                custom_name: "Idem Feed".to_owned(),
                default_name: "Idem Feed".to_owned(),
                category: "Official".to_owned(),
                handle_or_url: feed_url,
                config_json_enc: Some(enc),
                enabled: true,
            },
        )
        .await
        .unwrap();

    let date = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
    let first = orchestrator
        .compose_user_digest_inline(user_id, date, RunType::ManualDaily, false, false)
        .await
        .unwrap();
    assert!(first.run.prompt_tokens > 0);
    assert!(first.run.completion_tokens > 0);
    let llm_calls_after_first = llm_server.received_requests().await.unwrap().len();
    assert!(llm_calls_after_first > 0);

    // Second run with unchanged items/model/prompt hits fingerprint cache -> 0 duplicate LLM calls or tokens.
    let second = orchestrator
        .compose_user_digest_inline(user_id, date, RunType::ManualDaily, false, false)
        .await
        .unwrap();
    assert_eq!(second.run.prompt_tokens, 0);
    assert_eq!(second.run.completion_tokens, 0);
    assert_eq!(
        second.digest.as_ref().unwrap().id,
        first.digest.as_ref().unwrap().id
    );
    assert_eq!(
        llm_server.received_requests().await.unwrap().len(),
        llm_calls_after_first
    );
}

#[tokio::test]
async fn alert_scanner_emits_sse_dispatches_webhook_and_deduplicates_via_alerted_at() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/alert-feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Alerts</title><item><guid>al-1</guid><title>Critical gemini-urgent Security Patch</title><link>https://example.com/alerts/1</link><description>Immediate upgrade required.</description></item></channel></rss>"#,
        ))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/alert-hook"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&server)
        .await;

    let (repos, deps, _) = build_scheduler_harness();
    let user_id = create_test_user(&repos, "alert-user").await;

    repos
        .settings
        .upsert_schedule(
            user_id,
            UpdateScheduleCmd {
                timezone: "UTC".to_owned(),
                daily_time: "23:59".to_owned(),
                email_enabled: true,
                send_when_empty: false,
                alert_scan_enabled: true,
                alert_interval_minutes: 30,
            },
        )
        .await
        .unwrap();

    repos
        .settings
        .create_watchlist_rule(
            user_id,
            UpsertWatchlistRuleCmd {
                name: "Urgent Gemini Rule".to_owned(),
                keywords: vec!["gemini-urgent".to_owned()],
                exclude_keywords: Vec::new(),
                priority: AlertPriority::Urgent,
                notify_immediately: true,
                enabled: true,
            },
        )
        .await
        .unwrap();

    let hook_url = format!("{}/alert-hook", server.uri());
    let hook_enc = deps
        .cipher
        .encrypt(&serde_json::json!({ "url": hook_url }).to_string())
        .unwrap();
    repos
        .settings
        .create_channel(
            user_id,
            CreateChannelRecord {
                channel_type: NotificationChannelType::Webhook,
                name: "Alert Hook".to_owned(),
                enabled: true,
                config_enc: hook_enc,
                config_hint: "http://127.0.0.1/***".to_owned(),
                trigger_on_daily: false,
                trigger_on_alert: true,
            },
        )
        .await
        .unwrap();

    let feed_url = format!("{}/alert-feed.xml", server.uri());
    let feed_enc = encode_source_config(&deps.cipher, &feed_url, None).unwrap();
    repos
        .sources
        .create_or_subscribe(
            user_id,
            SubscribeSourceCmd {
                source_type: SourceType::Rss,
                scope: SourceScope::SharedPublic,
                canonical_key: "alert-rss".to_owned(),
                custom_name: "Alert RSS".to_owned(),
                default_name: "Alert RSS".to_owned(),
                category: "Official".to_owned(),
                handle_or_url: feed_url,
                config_json_enc: Some(feed_enc),
                enabled: true,
            },
        )
        .await
        .unwrap();

    let mut sse_rx = deps.sse.subscribe(user_id);
    let now = Utc::now();
    let tick1 = run_alert_scan_tick_at(&deps, now).await.unwrap();
    assert_eq!(tick1.users_scanned, 1);
    assert_eq!(tick1.alerts_triggered, 1);
    assert_eq!(tick1.deliveries_attempted, 1);

    let event = tokio::time::timeout(Duration::from_secs(2), sse_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.event_type, SseEventType::AlertTriggered);
    assert_eq!(event.payload["count"], 1);
    assert_eq!(
        event.payload["items"][0]["matchedKeywords"][0],
        "gemini-urgent"
    );
    assert!(repos
        .sources
        .list_pending_urgent_alerts(user_id)
        .await
        .unwrap()
        .is_empty());

    // Second alert scan past interval does NOT re-send already alerted item.
    let tick2 = run_alert_scan_tick_at(&deps, now + ChronoDuration::minutes(90))
        .await
        .unwrap();
    assert_eq!(tick2.users_scanned, 1);
    assert_eq!(tick2.alerts_triggered, 0);
    assert_eq!(tick2.deliveries_attempted, 0);
}

#[tokio::test]
async fn retention_pruner_removes_180_day_unbookmarked_bodies_while_preserving_bookmarked() {
    let (repos, deps, _) = build_scheduler_harness();
    let user_id = create_test_user(&repos, "ret-sched-user").await;

    let url = "https://93.184.216.34/ret.xml".to_owned();
    let enc = encode_source_config(&deps.cipher, &url, None).unwrap();
    let src = repos
        .sources
        .create_or_subscribe(
            user_id,
            SubscribeSourceCmd {
                source_type: SourceType::Rss,
                scope: SourceScope::SharedPublic,
                canonical_key: "sched-retention-rss".to_owned(),
                custom_name: "Retention".to_owned(),
                default_name: "Retention".to_owned(),
                category: "Official".to_owned(),
                handle_or_url: url,
                config_json_enc: Some(enc),
                enabled: true,
            },
        )
        .await
        .unwrap();

    let inserted = repos
        .sources
        .upsert_canonical_items(
            src.id,
            &[
                NewCanonicalItem {
                    external_id: Some("r1".to_owned()),
                    canonical_url: "https://example.com/r/1".to_owned(),
                    content_hash: "r-hash-1".to_owned(),
                    title: "Kept Item".to_owned(),
                    author: None,
                    summary_raw: Some("Kept summary body".to_owned()),
                    published_at: Some(Utc::now()),
                },
                NewCanonicalItem {
                    external_id: Some("r2".to_owned()),
                    canonical_url: "https://example.com/r/2".to_owned(),
                    content_hash: "r-hash-2".to_owned(),
                    title: "Pruned Item".to_owned(),
                    author: None,
                    summary_raw: Some("Ephemeral summary body".to_owned()),
                    published_at: Some(Utc::now()),
                },
            ],
        )
        .await
        .unwrap();

    repos
        .sources
        .upsert_bookmark(user_id, inserted[0].id, Some("Keep".to_owned()))
        .await
        .unwrap();

    assert_eq!(
        run_retention_once(&repos, 180, Utc::now()).await.unwrap(),
        0
    );
    assert_eq!(
        run_retention_once(&repos, 180, Utc::now() + ChronoDuration::days(181))
            .await
            .unwrap(),
        1
    );

    let views = repos
        .sources
        .list_source_items(user_id, src.id, PageRequest::default())
        .await
        .unwrap();
    let kept = views.items.iter().find(|i| i.id == inserted[0].id).unwrap();
    let pruned = views.items.iter().find(|i| i.id == inserted[1].id).unwrap();
    assert!(!kept.body_pruned);
    assert_eq!(kept.summary.as_deref(), Some("Kept summary body"));
    assert!(pruned.body_pruned);
    assert!(pruned.summary.is_none());
}
