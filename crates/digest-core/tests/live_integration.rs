//! Live end-to-end integration test exercising OpenAI-compatible LLM connectivity,
//! `DigestExecutionWorkflow`, fingerprint idempotency caching, and live SMTP delivery.
//!
//! Marked `#[ignore]` so `cargo test --workspace` stays offline-safe in CI.
//! Run explicitly with:
//! `cargo test -p digest-core --test live_integration -- --ignored --nocapture`

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use digest_core::cache::InMemoryCacheCoordinator;
use digest_core::collectors::{CollectorContext, CollectorRegistry};
use digest_core::config::{CollectorTuning, LlmTuning, NotifierTuning, ResourceLimitsTuning};
use digest_core::constants::ENV_SSRF_ALLOWLIST_HOSTS;
use digest_core::domain::{
    AlertPriority, DeliveryStatus, DigestWorkflowInput, NotificationChannelType, NotifierConfig,
    RunStatus, RunType, SourceRunInput, SourceScope, SourceType, UserAiConfig, WatchlistRuleInput,
};
use digest_core::error::CoreError;
use digest_core::http::shared_http_client;
use digest_core::llm::OpenAiCompatProvider;
use digest_core::notifiers::NotifierRegistry;
use digest_core::ssrf::SsrfGuard;
use digest_core::traits::{ConditionalFetchHeaders, SourceDescriptor, StructuredLlmProvider};
use digest_core::workflow::DigestExecutionWorkflow;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DEFAULT_SECRET_PATH: &str = r"D:\work\git hub work\vps-us\docs\gemini-digest-test.secret";
const ENV_SECRET_PATH: &str = "GEMINI_DIGEST_TEST_SECRET_PATH";

const LIVE_TEST_RSS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Gemini Ecosystem Live Verification Feed</title>
    <item>
      <guid>gemini-live-1</guid>
      <title>Gemini 2.5 Pro Reasoning and Context Caching Update</title>
      <link>https://blog.google/technology/ai/gemini-2-5-pro-update/?utm_source=digest-test</link>
      <description>Google announced expanded context caching and improved coding benchmarks for Gemini 2.5 Pro.</description>
      <pubDate>Thu, 26 Nov 2026 01:00:00 +0000</pubDate>
    </item>
    <item>
      <guid>gemini-live-2</guid>
      <title>Google GenAI Python SDK v1.1.0 Released</title>
      <link>https://github.com/googleapis/python-genai/releases/tag/v1.1.0</link>
      <description>Adds typed streaming tool calls and improved retry diagnostics for Gemini API developers.</description>
      <pubDate>Thu, 26 Nov 2026 02:00:00 +0000</pubDate>
    </item>
  </channel>
</rss>"#;

fn load_secret_map() -> Result<HashMap<String, String>, CoreError> {
    let path = std::env::var(ENV_SECRET_PATH)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SECRET_PATH));

    let raw = std::fs::read_to_string(&path).map_err(|_| {
        CoreError::InvalidConfig("live integration secret file could not be read".to_owned())
    })?;

    let mut map = HashMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            let k = key.trim().to_owned();
            let v = value.trim().to_owned();
            if !k.is_empty() && !v.is_empty() {
                map.insert(k, v);
            }
        }
    }
    Ok(map)
}

fn require_secret(map: &HashMap<String, String>, key: &str) -> Result<String, CoreError> {
    map.get(key)
        .cloned()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| CoreError::InvalidConfig(format!("missing required secret key: {key}")))
}

#[tokio::test]
#[ignore = "requires local gemini-digest-test.secret"]
async fn live_deepseek_workflow_cache_and_smtp_delivery() -> Result<(), CoreError> {
    let secrets = load_secret_map()?;
    let deepseek_base_url = require_secret(&secrets, "DEEPSEEK_BASE_URL")?;
    let deepseek_model = require_secret(&secrets, "DEEPSEEK_MODEL")?;
    let deepseek_api_key = require_secret(&secrets, "DEEPSEEK_API_KEY")?;
    let smtp_host = require_secret(&secrets, "SMTP_HOST")?;
    let smtp_port_raw = require_secret(&secrets, "SMTP_PORT")?;
    let smtp_port: u16 = smtp_port_raw
        .parse()
        .map_err(|_| CoreError::InvalidConfig("invalid SMTP_PORT".to_owned()))?;
    let smtp_username = require_secret(&secrets, "SMTP_USERNAME")?;
    let smtp_password = require_secret(&secrets, "SMTP_APP_PASSWORD")?;
    let smtp_to = require_secret(&secrets, "SMTP_TO")?;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/gemini-live.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(LIVE_TEST_RSS_XML))
        .mount(&server)
        .await;

    let collector_tuning = CollectorTuning {
        initial_backoff: Duration::from_millis(50),
        ..CollectorTuning::default()
    };
    let limits = ResourceLimitsTuning::default();
    let http_client = shared_http_client(None, &collector_tuning)?;

    // Allow loopback for local wiremock and explicitly configured live endpoints
    // (supports local Fake-IP TUN DNS resolvers returning 198.18.0.0/15).
    let mut allowlist_hosts = vec!["127.0.0.1".to_owned(), "localhost".to_owned()];
    if let Ok(parsed_llm_url) = url::Url::parse(&deepseek_base_url) {
        if let Some(host) = parsed_llm_url.host_str() {
            allowlist_hosts.push(host.to_owned());
        }
    }
    allowlist_hosts.push(smtp_host.clone());
    if let Ok(extra_allowlist) = std::env::var(ENV_SSRF_ALLOWLIST_HOSTS) {
        for host in extra_allowlist
            .split(',')
            .map(str::trim)
            .filter(|h| !h.is_empty())
        {
            allowlist_hosts.push(host.to_owned());
        }
    }
    let ssrf = SsrfGuard::new(allowlist_hosts);

    let llm_provider = Arc::new(OpenAiCompatProvider::with_tuning(
        http_client.clone(),
        ssrf.clone(),
        LlmTuning::default(),
        limits.max_feed_bytes.get(),
    ));

    let ai_config = UserAiConfig {
        provider_label: "DeepSeek".to_owned(),
        base_url: deepseek_base_url,
        api_key: Some(deepseek_api_key),
        model: deepseek_model,
        system_prompt:
            "Summarize Gemini ecosystem updates concisely in Markdown using only the provided items."
                .to_owned(),
        temperature: 0.2,
        max_tokens: 1024,
        ..UserAiConfig::default()
    };

    // 1. Live LLM connectivity check.
    llm_provider
        .test_connection(&ai_config)
        .await
        .map_err(|err| CoreError::Upstream(format!("live LLM test_connection failed: {err}")))?;

    // 2. Full DigestExecutionWorkflow run against live DeepSeek + live SMTP.
    let collector_ctx = CollectorContext::new(
        http_client.clone(),
        ssrf.clone(),
        collector_tuning,
        limits.clone(),
    );
    let collectors = CollectorRegistry::new(collector_ctx);
    let notifiers = NotifierRegistry::with_defaults(
        http_client,
        ssrf,
        NotifierTuning {
            smtp_timeout: Duration::from_secs(30),
            ..NotifierTuning::default()
        },
    );
    let cache = Arc::new(InMemoryCacheCoordinator::new());
    let workflow =
        DigestExecutionWorkflow::with_defaults(collectors, llm_provider, notifiers, cache, limits);

    let input = DigestWorkflowInput {
        user_id: Some(Uuid::new_v4()),
        digest_date: Utc::now().date_naive(),
        run_type: RunType::ManualDaily,
        sources: vec![SourceRunInput {
            source: SourceDescriptor {
                id: Uuid::new_v4(),
                source_type: SourceType::Rss,
                scope: SourceScope::SharedPublic,
                canonical_key: "rss:live-test-gemini".to_owned(),
                name: "Gemini Ecosystem Test Feed".to_owned(),
                handle_or_url: format!("{}/gemini-live.xml", server.uri()),
                category: "Official".to_owned(),
            },
            conditional: ConditionalFetchHeaders::default(),
            credentials_json: None,
        }],
        watchlist_rules: vec![WatchlistRuleInput {
            id: None,
            name: "Gemini 2.5".to_owned(),
            keywords: vec!["Gemini 2.5".to_owned()],
            exclude_keywords: Vec::new(),
            priority: AlertPriority::Urgent,
            notify_immediately: true,
            enabled: true,
        }],
        ai_config,
        notifiers: vec![NotifierConfig {
            channel_id: Some(Uuid::new_v4()),
            channel_type: NotificationChannelType::Smtp,
            name: "Live SMTP Channel".to_owned(),
            config_json: serde_json::json!({
                "host": smtp_host,
                "port": smtp_port,
                "username": smtp_username,
                "password": smtp_password,
                "from": smtp_username,
                "to": [smtp_to]
            }),
        }],
        send_when_empty: false,
        force_refresh: false,
    };

    let first = workflow.execute(&input).await?;
    let delivery_status = first
        .deliveries
        .first()
        .map(|d| d.status.as_str())
        .unwrap_or("none");

    println!(
        "live_run_1: status={} items_fetched={} prompt_tokens={} completion_tokens={} degraded_fallback={} delivery_status={}",
        first.status,
        first.items_fetched,
        first.prompt_tokens,
        first.completion_tokens,
        first.summarized_digest.degraded_fallback,
        delivery_status
    );

    if first.status != RunStatus::Succeeded {
        return Err(CoreError::Upstream(format!(
            "expected live run status succeeded, got {} (delivery_status={delivery_status})",
            first.status
        )));
    }
    if first.summarized_digest.degraded_fallback {
        return Err(CoreError::Upstream(
            "expected live DeepSeek summary without degraded fallback".to_owned(),
        ));
    }
    if first.prompt_tokens == 0 || first.completion_tokens == 0 {
        return Err(CoreError::Upstream(
            "expected non-zero live LLM token counts on first run".to_owned(),
        ));
    }
    if first.deliveries.first().map(|d| d.status) != Some(DeliveryStatus::Sent) {
        return Err(CoreError::Upstream(
            "expected live SMTP delivery status sent".to_owned(),
        ));
    }

    // 3. Second run with identical input_fingerprint must hit cache with 0 tokens.
    let second_input = DigestWorkflowInput {
        notifiers: Vec::new(),
        ..input
    };
    let second = workflow.execute(&second_input).await?;

    println!(
        "live_run_2: status={} cache_hit={} prompt_tokens={} completion_tokens={} fingerprint_match={}",
        second.status,
        second.cache_hit,
        second.prompt_tokens,
        second.completion_tokens,
        second.input_fingerprint == first.input_fingerprint
    );

    if !second.cache_hit || second.prompt_tokens != 0 || second.completion_tokens != 0 {
        return Err(CoreError::Internal(
            "expected second identical run to hit fingerprint cache with 0 tokens".to_owned(),
        ));
    }
    if second.input_fingerprint != first.input_fingerprint {
        return Err(CoreError::Internal(
            "expected identical input_fingerprint across identical runs".to_owned(),
        ));
    }

    Ok(())
}
