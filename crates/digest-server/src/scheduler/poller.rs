//! Layer 1 — Global adaptive canonical pool poller with zero-subscriber sleep,
//! `CacheCoordinator` singleflight locking, freshness TTL, conditional HTTP 304,
//! and exponential failure backoff.

use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use digest_core::cache::CacheCoordinator;
use digest_core::collectors::{next_failure_backoff, CollectorRegistry};
use digest_core::config::{CollectorTuning, ResourceLimitsTuning};
use digest_core::constants::SECRET_MASK_REDACTED_PLACEHOLDER;
use digest_core::crypto::EnvelopeCipher;
use digest_core::domain::{FetchOutcome, SourceScope, SourceType};
use digest_core::error::CoreError;
use digest_core::pipeline::ItemProcessingPipeline;
use digest_core::presets::find_preset_by_key;
use digest_core::traits::{
    ConditionalFetchHeaders, FetchResult, SourceDescriptor, StructuredItemProcessor,
};
use futures::stream::{self, StreamExt};
use uuid::Uuid;

use crate::constants::SCHEDULER_POLL_BATCH_LIMIT;
use crate::repo::{
    decode_source_config, CanonicalItemRecord, CanonicalSourceRecord, NewCanonicalItem,
    Repositories, SourceConfigPayload, SourceFetchStateUpdate, UserSourceRecord,
};

const DEFAULT_SOURCE_CATEGORY: &str = "General";

/// Effective platform-wide X integration settings used as fallback for X sources.
#[derive(Clone, Default)]
pub struct EffectivePlatformXConfig {
    pub bridge_base_url: Option<String>,
    pub bearer_token_enc: Option<String>,
}

impl std::fmt::Debug for EffectivePlatformXConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = self
            .bearer_token_enc
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("EffectivePlatformXConfig")
            .field("bridge_base_url", &self.bridge_base_url)
            .field("bearer_token_enc", &redacted)
            .finish()
    }
}

impl EffectivePlatformXConfig {
    /// Constructs an `EffectivePlatformXConfig` trimming empty strings to `None`.
    #[must_use]
    pub fn from_parts(bridge_base_url: Option<String>, bearer_token_enc: Option<String>) -> Self {
        let normalize =
            |opt: Option<String>| opt.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        Self {
            bridge_base_url: normalize(bridge_base_url),
            bearer_token_enc: normalize(bearer_token_enc),
        }
    }
}

/// Redacts any secret strings before surfacing upstream error messages.
#[must_use]
pub fn sanitize_x_platform_error<'a>(
    raw: &str,
    secrets: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut sanitized = raw.to_owned();
    for secret in secrets {
        let trimmed = secret.trim();
        if !trimmed.is_empty() {
            sanitized = sanitized.replace(trimmed, SECRET_MASK_REDACTED_PLACEHOLDER);
        }
    }
    sanitized
}

/// Resolves effective platform X config from `platform_settings` DB row + optional env fallback.
pub async fn resolve_effective_platform_x_config(
    repos: &Repositories,
    env_bridge_base_url: Option<&str>,
) -> Result<EffectivePlatformXConfig, CoreError> {
    let stored = repos.settings.get_platform_settings().await?;
    let bridge = stored
        .as_ref()
        .and_then(|s| s.x_bridge_base_url.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            env_bridge_base_url
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        });
    let bearer_enc = stored.and_then(|s| s.x_bearer_token_enc);
    Ok(EffectivePlatformXConfig::from_parts(bridge, bearer_enc))
}

fn has_per_source_bearer_token(credentials: Option<&serde_json::Value>) -> bool {
    let Some(obj) = credentials.and_then(|v| v.as_object()) else {
        return false;
    };
    ["bearer_token", "bearerToken", "access_token", "token"]
        .iter()
        .any(|key| {
            obj.get(*key)
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.trim().is_empty())
        })
}

fn extract_x_handle_for_bridge(handle_or_url: &str) -> Result<String, CoreError> {
    let trimmed = handle_or_url.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(
            "X account handle must not be empty".to_owned(),
        ));
    }
    let handle = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let parsed = url::Url::parse(trimmed)
            .map_err(|err| CoreError::InvalidInput(format!("invalid X account URL: {err}")))?;
        parsed
            .path_segments()
            .and_then(|mut segs| segs.find(|s| !s.is_empty()))
            .unwrap_or("")
            .trim_start_matches('@')
            .to_owned()
    } else {
        trimmed.trim_start_matches('@').to_owned()
    };

    if handle.is_empty()
        || !handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(CoreError::InvalidInput(format!(
            "invalid X username handle: {trimmed}"
        )));
    }
    Ok(handle)
}

fn build_bridge_rss_url(
    base_url: &str,
    source_type: SourceType,
    handle_or_url: &str,
) -> Result<String, CoreError> {
    let base = base_url.trim().trim_end_matches('/');
    match source_type {
        SourceType::XAccount => {
            let handle = extract_x_handle_for_bridge(handle_or_url)?;
            Ok(format!("{base}/twitter/user/{handle}"))
        }
        SourceType::XSearch => {
            let query = handle_or_url.trim();
            if query.is_empty() {
                return Err(CoreError::InvalidInput(
                    "X search query must not be empty".to_owned(),
                ));
            }
            let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes())
                .map(|chunk| chunk.replace('+', "%20"))
                .collect();
            Ok(format!("{base}/twitter/keyword/{encoded}"))
        }
        other => Err(CoreError::InvalidInput(format!(
            "source type {other} is not an X source"
        ))),
    }
}

/// Fetches a `SourceDescriptor`, routing X sources with missing per-source bearer tokens
/// through the admin-configured X Bridge (RSSHub) or platform bearer token when present.
#[allow(clippy::too_many_arguments)]
pub async fn fetch_with_platform_x_support(
    repos: &Repositories,
    cipher: &Arc<EnvelopeCipher>,
    collectors: &Arc<CollectorRegistry>,
    descriptor: &SourceDescriptor,
    conditional: &ConditionalFetchHeaders,
    per_source_credentials: Option<&serde_json::Value>,
    platform_override: Option<&EffectivePlatformXConfig>,
    env_bridge_base_url: Option<&str>,
) -> Result<FetchResult, CoreError> {
    if !matches!(
        descriptor.source_type,
        SourceType::XAccount | SourceType::XSearch
    ) || has_per_source_bearer_token(per_source_credentials)
    {
        return collectors
            .fetch(descriptor, conditional, per_source_credentials)
            .await;
    }

    let effective = match platform_override {
        Some(cfg) => cfg.clone(),
        None => resolve_effective_platform_x_config(repos, env_bridge_base_url).await?,
    };

    if let Some(ref bridge_base) = effective.bridge_base_url {
        let rss_url = build_bridge_rss_url(
            bridge_base,
            descriptor.source_type,
            &descriptor.handle_or_url,
        )?;
        let bridge_descriptor = SourceDescriptor {
            source_type: SourceType::Rss,
            handle_or_url: rss_url,
            ..descriptor.clone()
        };
        return collectors
            .fetch(&bridge_descriptor, conditional, None)
            .await;
    }

    if let Some(enc_token) = effective.bearer_token_enc {
        let cipher_cloned = Arc::clone(cipher);
        let token = tokio::task::spawn_blocking(move || cipher_cloned.decrypt(&enc_token))
            .await
            .map_err(|err| {
                CoreError::Internal(format!("platform X token decrypt failed: {err}"))
            })??;
        let creds = serde_json::json!({ "bearer_token": token });
        return collectors
            .fetch(descriptor, conditional, Some(&creds))
            .await;
    }

    Err(CoreError::Unauthorized(
        "X source requires per-source API credentials or administrator-configured platform X integration".to_owned(),
    ))
}

/// Outcome of polling a single canonical source.
#[derive(Debug, Clone)]
pub struct SourcePollOutcome {
    pub source_id: Uuid,
    pub outcome: FetchOutcome,
    pub items_fetched: usize,
    pub stored_items: Vec<CanonicalItemRecord>,
    pub error_message: Option<String>,
}

/// Summary of a single global poller tick.
#[derive(Debug, Clone, Default)]
pub struct PollerTickReport {
    pub sources_polled: usize,
    pub updated: usize,
    pub not_modified: usize,
    pub skipped_fresh: usize,
    pub failed: usize,
    pub items_fetched: usize,
}

/// Converts a joined `UserSourceRecord` into a `CanonicalSourceRecord` for canonical pooling.
#[must_use]
pub fn canonical_source_from_user_source(user_source: &UserSourceRecord) -> CanonicalSourceRecord {
    CanonicalSourceRecord {
        id: user_source.canonical_source_id,
        source_type: user_source.source_type,
        canonical_key: user_source.canonical_key.clone(),
        scope: user_source.scope,
        scope_owner_id: user_source.scope_owner_id,
        default_name: user_source.default_name.clone(),
        config_json_enc: user_source.config_json_enc.clone(),
        etag: user_source.etag.clone(),
        last_modified: user_source.last_modified.clone(),
        last_fetched_at: user_source.last_fetched_at,
        next_poll_at: user_source.next_poll_at,
        last_status: user_source.last_status,
        last_error: user_source.last_error.clone(),
        consecutive_failures: user_source.consecutive_failures,
        created_at: user_source.created_at,
    }
}

/// Executes one Layer 1 poller tick across due canonical sources with enabled subscribers.
pub async fn run_poller_tick(
    repos: &Repositories,
    cache: &Arc<dyn CacheCoordinator>,
    cipher: &Arc<EnvelopeCipher>,
    collectors: &Arc<CollectorRegistry>,
    pipeline: &Arc<ItemProcessingPipeline>,
    collector_tuning: &CollectorTuning,
    limits: &ResourceLimitsTuning,
) -> Result<PollerTickReport, CoreError> {
    run_poller_tick_with_platform_x(
        repos,
        cache,
        cipher,
        collectors,
        pipeline,
        collector_tuning,
        limits,
        None,
    )
    .await
}

/// Executes one Layer 1 poller tick with an optional env-level X Bridge base URL fallback.
#[allow(clippy::too_many_arguments)]
pub async fn run_poller_tick_with_platform_x(
    repos: &Repositories,
    cache: &Arc<dyn CacheCoordinator>,
    cipher: &Arc<EnvelopeCipher>,
    collectors: &Arc<CollectorRegistry>,
    pipeline: &Arc<ItemProcessingPipeline>,
    collector_tuning: &CollectorTuning,
    limits: &ResourceLimitsTuning,
    env_bridge_base_url: Option<&str>,
) -> Result<PollerTickReport, CoreError> {
    let now = Utc::now();
    let due_sources = repos
        .sources
        .list_pollable_sources(None, now, SCHEDULER_POLL_BATCH_LIMIT)
        .await?;

    if due_sources.is_empty() {
        return Ok(PollerTickReport::default());
    }

    let env_bridge_owned = env_bridge_base_url.map(str::to_owned);
    let concurrency = limits.global_fetch_concurrency.get().max(1);
    let outcomes: Vec<SourcePollOutcome> = stream::iter(due_sources)
        .map(|source| {
            let repos = repos.clone();
            let cache = Arc::clone(cache);
            let cipher = Arc::clone(cipher);
            let collectors = Arc::clone(collectors);
            let pipeline = Arc::clone(pipeline);
            let collector_tuning = collector_tuning.clone();
            let limits = limits.clone();
            let env_bridge = env_bridge_owned.clone();
            async move {
                match poll_canonical_source_once_with_platform_x(
                    &repos,
                    &cache,
                    &cipher,
                    &collectors,
                    &pipeline,
                    &collector_tuning,
                    &limits,
                    &source,
                    false,
                    env_bridge.as_deref(),
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(err) => {
                        tracing::warn!(
                            source_id = %source.id,
                            source_type = %source.source_type,
                            error = %err,
                            "canonical source poll failed unexpectedly"
                        );
                        SourcePollOutcome {
                            source_id: source.id,
                            outcome: FetchOutcome::Failed,
                            items_fetched: 0,
                            stored_items: Vec::new(),
                            error_message: Some(err.to_string()),
                        }
                    }
                }
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let mut report = PollerTickReport {
        sources_polled: outcomes.len(),
        ..PollerTickReport::default()
    };
    for outcome in outcomes {
        report.items_fetched = report.items_fetched.saturating_add(outcome.items_fetched);
        match outcome.outcome {
            FetchOutcome::Updated => report.updated += 1,
            FetchOutcome::NotModified304 => report.not_modified += 1,
            FetchOutcome::SkippedFresh => report.skipped_fresh += 1,
            FetchOutcome::Failed => report.failed += 1,
        }
    }

    Ok(report)
}

/// Polls a single canonical source while enforcing `CacheCoordinator` singleflight locking,
/// freshness TTL, `ItemProcessingPipeline` normalization/dedup, and adaptive failure backoff.
#[allow(clippy::too_many_arguments)]
pub async fn poll_canonical_source_once(
    repos: &Repositories,
    cache: &Arc<dyn CacheCoordinator>,
    cipher: &Arc<EnvelopeCipher>,
    collectors: &Arc<CollectorRegistry>,
    pipeline: &Arc<ItemProcessingPipeline>,
    collector_tuning: &CollectorTuning,
    limits: &ResourceLimitsTuning,
    source: &CanonicalSourceRecord,
    force_refresh: bool,
) -> Result<SourcePollOutcome, CoreError> {
    poll_canonical_source_once_with_platform_x(
        repos,
        cache,
        cipher,
        collectors,
        pipeline,
        collector_tuning,
        limits,
        source,
        force_refresh,
        None,
    )
    .await
}

/// Polls a single canonical source with an optional env-level X Bridge base URL fallback.
#[allow(clippy::too_many_arguments)]
pub async fn poll_canonical_source_once_with_platform_x(
    repos: &Repositories,
    cache: &Arc<dyn CacheCoordinator>,
    cipher: &Arc<EnvelopeCipher>,
    collectors: &Arc<CollectorRegistry>,
    pipeline: &Arc<ItemProcessingPipeline>,
    collector_tuning: &CollectorTuning,
    limits: &ResourceLimitsTuning,
    source: &CanonicalSourceRecord,
    force_refresh: bool,
    env_bridge_base_url: Option<&str>,
) -> Result<SourcePollOutcome, CoreError> {
    let cache_key = format!("source:{}", source.id);

    if !force_refresh {
        match cache.is_fresh(&cache_key).await {
            Ok(true) => {
                return Ok(SourcePollOutcome {
                    source_id: source.id,
                    outcome: FetchOutcome::SkippedFresh,
                    items_fetched: 0,
                    stored_items: Vec::new(),
                    error_message: None,
                });
            }
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(
                    source_id = %source.id,
                    error = %err,
                    "cache freshness check failed; proceeding with poll"
                );
            }
        }
    }

    let lock_acquired = match cache
        .try_acquire_lock(&cache_key, limits.singleflight_lock_ttl)
        .await
    {
        Ok(acquired) => acquired,
        Err(err) => {
            tracing::warn!(
                source_id = %source.id,
                error = %err,
                "singleflight lock acquisition failed; proceeding"
            );
            true
        }
    };

    if !lock_acquired {
        return Ok(SourcePollOutcome {
            source_id: source.id,
            outcome: FetchOutcome::SkippedFresh,
            items_fetched: 0,
            stored_items: Vec::new(),
            error_message: None,
        });
    }

    let result = poll_locked_source(
        repos,
        cache,
        cipher,
        collectors,
        pipeline,
        collector_tuning,
        limits,
        source,
        &cache_key,
        force_refresh,
        env_bridge_base_url,
    )
    .await;

    if let Err(err) = cache.release_lock(&cache_key).await {
        tracing::warn!(
            source_id = %source.id,
            error = %err,
            "failed to release singleflight lock"
        );
    }

    result
}

#[allow(clippy::too_many_arguments)]
async fn poll_locked_source(
    repos: &Repositories,
    cache: &Arc<dyn CacheCoordinator>,
    cipher: &Arc<EnvelopeCipher>,
    collectors: &Arc<CollectorRegistry>,
    pipeline: &Arc<ItemProcessingPipeline>,
    collector_tuning: &CollectorTuning,
    limits: &ResourceLimitsTuning,
    source: &CanonicalSourceRecord,
    cache_key: &str,
    force_refresh: bool,
    env_bridge_base_url: Option<&str>,
) -> Result<SourcePollOutcome, CoreError> {
    if !force_refresh && cache.is_fresh(cache_key).await.unwrap_or(false) {
        return Ok(SourcePollOutcome {
            source_id: source.id,
            outcome: FetchOutcome::SkippedFresh,
            items_fetched: 0,
            stored_items: Vec::new(),
            error_message: None,
        });
    }

    let payload = decode_source_payload_blocking(
        Arc::clone(cipher),
        source.canonical_key.clone(),
        source.config_json_enc.clone(),
    )
    .await?;

    let category = find_preset_by_key(&source.canonical_key)
        .map(|preset| preset.category)
        .unwrap_or_else(|| DEFAULT_SOURCE_CATEGORY.to_owned());

    let descriptor = SourceDescriptor {
        id: source.id,
        source_type: source.source_type,
        scope: source.scope,
        canonical_key: source.canonical_key.clone(),
        name: source.default_name.clone(),
        handle_or_url: payload.handle_or_url,
        category,
    };

    let conditional = ConditionalFetchHeaders {
        etag: source.etag.clone(),
        last_modified: source.last_modified.clone(),
    };

    let credentials = if source.scope == SourceScope::UserPrivate || payload.credentials.is_some() {
        payload.credentials.as_ref()
    } else {
        None
    };

    let fetch_res = fetch_with_platform_x_support(
        repos,
        cipher,
        collectors,
        &descriptor,
        &conditional,
        credentials,
        None,
        env_bridge_base_url,
    )
    .await;
    let now = Utc::now();
    let freshness_chrono = ChronoDuration::from_std(limits.source_freshness_ttl)
        .unwrap_or_else(|_| ChronoDuration::seconds(900));

    match fetch_res {
        Ok(fetch_result) => {
            if fetch_result.outcome == FetchOutcome::NotModified304 {
                if let Err(err) = cache
                    .mark_fresh(cache_key, limits.source_freshness_ttl)
                    .await
                {
                    tracing::warn!(source_id = %source.id, error = %err, "failed to mark source fresh");
                }

                let update = SourceFetchStateUpdate {
                    outcome: FetchOutcome::NotModified304,
                    etag: fetch_result.etag.or_else(|| source.etag.clone()),
                    last_modified: fetch_result
                        .last_modified
                        .or_else(|| source.last_modified.clone()),
                    fetched_at: now,
                    next_poll_at: Some(now + freshness_chrono),
                    last_error: None,
                    consecutive_failures: 0,
                };
                repos
                    .sources
                    .record_fetch_outcome(source.id, update)
                    .await?;

                return Ok(SourcePollOutcome {
                    source_id: source.id,
                    outcome: FetchOutcome::NotModified304,
                    items_fetched: 0,
                    stored_items: Vec::new(),
                    error_message: None,
                });
            }

            let raw_count = fetch_result.items.len();
            let processed = pipeline
                .process_items(Some(&descriptor), fetch_result.items, &[])
                .await?;

            let new_items: Vec<NewCanonicalItem> = processed
                .into_iter()
                .map(|item| NewCanonicalItem {
                    external_id: item.external_id,
                    canonical_url: item.canonical_url,
                    content_hash: item.content_hash,
                    title: item.title,
                    author: item.author,
                    summary_raw: item.summary,
                    published_at: item.published_at,
                })
                .collect();

            let stored_items = if new_items.is_empty() {
                Vec::new()
            } else {
                repos
                    .sources
                    .upsert_canonical_items(source.id, &new_items)
                    .await?
            };

            if let Err(err) = cache
                .mark_fresh(cache_key, limits.source_freshness_ttl)
                .await
            {
                tracing::warn!(source_id = %source.id, error = %err, "failed to mark source fresh");
            }

            let update = SourceFetchStateUpdate {
                outcome: FetchOutcome::Updated,
                etag: fetch_result.etag.or_else(|| source.etag.clone()),
                last_modified: fetch_result
                    .last_modified
                    .or_else(|| source.last_modified.clone()),
                fetched_at: now,
                next_poll_at: Some(now + freshness_chrono),
                last_error: None,
                consecutive_failures: 0,
            };
            repos
                .sources
                .record_fetch_outcome(source.id, update)
                .await?;

            Ok(SourcePollOutcome {
                source_id: source.id,
                outcome: FetchOutcome::Updated,
                items_fetched: raw_count,
                stored_items,
                error_message: None,
            })
        }
        Err(err) => {
            let next_failures = source.consecutive_failures.saturating_add(1).max(1);
            let failures_u32 = u32::try_from(next_failures).unwrap_or(u32::MAX);
            let backoff = next_failure_backoff(failures_u32, collector_tuning);
            let err_msg = err.to_string();

            let update = SourceFetchStateUpdate {
                outcome: FetchOutcome::Failed,
                etag: source.etag.clone(),
                last_modified: source.last_modified.clone(),
                fetched_at: now,
                next_poll_at: Some(now + backoff),
                last_error: Some(err_msg.clone()),
                consecutive_failures: next_failures,
            };
            repos
                .sources
                .record_fetch_outcome(source.id, update)
                .await?;

            Ok(SourcePollOutcome {
                source_id: source.id,
                outcome: FetchOutcome::Failed,
                items_fetched: 0,
                stored_items: Vec::new(),
                error_message: Some(err_msg),
            })
        }
    }
}

/// Offloads encrypted source config decryption to `spawn_blocking` when ciphertext is present.
pub async fn decode_source_payload_blocking(
    cipher: Arc<EnvelopeCipher>,
    canonical_key: String,
    config_json_enc: Option<String>,
) -> Result<SourceConfigPayload, CoreError> {
    if config_json_enc.is_some() {
        tokio::task::spawn_blocking(move || {
            decode_source_config(&cipher, &canonical_key, config_json_enc.as_deref())
        })
        .await
        .map_err(|err| CoreError::Internal(format!("source config decrypt task failed: {err}")))?
    } else {
        decode_source_config(&cipher, &canonical_key, None)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use digest_core::cache::InMemoryCacheCoordinator;
    use digest_core::collectors::CollectorContext;
    use digest_core::domain::SourceType;
    use digest_core::http::shared_http_client;
    use digest_core::ssrf::SsrfGuard;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;
    use crate::repo::{encode_source_config, SettingsRepo, SourceRepo, SubscribeSourceCmd};
    use crate::scheduler::test_support::MockRepos;

    fn test_pipeline() -> Arc<ItemProcessingPipeline> {
        Arc::new(ItemProcessingPipeline::new())
    }

    fn test_cipher() -> Arc<EnvelopeCipher> {
        Arc::new(EnvelopeCipher::from_bytes([0x11u8; 32]))
    }

    fn test_collectors() -> (
        Arc<CollectorRegistry>,
        CollectorTuning,
        ResourceLimitsTuning,
    ) {
        let tuning = CollectorTuning {
            max_retry_attempts: NonZeroUsize::new(1).unwrap(),
            initial_backoff: Duration::from_millis(5),
            ..CollectorTuning::default()
        };
        let limits = ResourceLimitsTuning::default();
        let client = shared_http_client(None, &tuning).unwrap();
        let ssrf = SsrfGuard::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()]);
        let ctx = CollectorContext::new(client, ssrf, tuning.clone(), limits.clone());
        (Arc::new(CollectorRegistry::new(ctx)), tuning, limits)
    }

    #[tokio::test]
    async fn zero_subscriber_source_sleeps_with_zero_network_calls() {
        let server = MockServer::start().await;
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_clone = Arc::clone(&hits);
        Mock::given(method("GET"))
            .and(path("/rss.xml"))
            .respond_with(move |_req: &Request| {
                hits_clone.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
            })
            .mount(&server)
            .await;

        let mock = MockRepos::new();
        let user_id = Uuid::new_v4();
        let cipher = test_cipher();
        let enc =
            encode_source_config(&cipher, &format!("{}/rss.xml", server.uri()), None).unwrap();
        // Subscribe with enabled = false -> 0 enabled subscribers.
        mock.sources
            .create_or_subscribe(
                user_id,
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "custom-sleep".to_owned(),
                    custom_name: "Sleep".to_owned(),
                    default_name: "Sleep".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: format!("{}/rss.xml", server.uri()),
                    config_json_enc: Some(enc),
                    enabled: false,
                },
            )
            .await
            .unwrap();

        let repos = mock.repositories();
        let cache: Arc<dyn CacheCoordinator> = Arc::new(InMemoryCacheCoordinator::new());
        let (collectors, tuning, limits) = test_collectors();
        let report = run_poller_tick(
            &repos,
            &cache,
            &cipher,
            &collectors,
            &test_pipeline(),
            &tuning,
            &limits,
        )
        .await
        .unwrap();

        assert_eq!(report.sources_polled, 0);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn poller_fetches_shared_source_once_supports_304_and_freshness_ttl() {
        let server = MockServer::start().await;
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_clone = Arc::clone(&hits);
        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .respond_with(move |req: &Request| {
                let count = hits_clone.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    ResponseTemplate::new(200)
                        .insert_header("etag", "\"v1\"")
                        .set_body_string(
                            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Feed</title><item><guid>g1</guid><title>Gemini 2.5</title><link>https://example.com/1?utm_source=x</link><description>Hello</description></item></channel></rss>"#,
                        )
                } else if req.headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some("\"v1\"") {
                    ResponseTemplate::new(304).insert_header("etag", "\"v1\"")
                } else {
                    ResponseTemplate::new(500)
                }
            })
            .mount(&server)
            .await;

        let mock = MockRepos::new();
        let cipher = test_cipher();
        let enc =
            encode_source_config(&cipher, &format!("{}/feed.xml", server.uri()), None).unwrap();

        // Two users subscribe to the exact same shared_public source.
        for _ in 0..2 {
            mock.sources
                .create_or_subscribe(
                    Uuid::new_v4(),
                    SubscribeSourceCmd {
                        source_type: SourceType::Rss,
                        scope: SourceScope::SharedPublic,
                        canonical_key: "shared-feed".to_owned(),
                        custom_name: "Shared".to_owned(),
                        default_name: "Shared".to_owned(),
                        category: "Official".to_owned(),
                        handle_or_url: format!("{}/feed.xml", server.uri()),
                        config_json_enc: Some(enc.clone()),
                        enabled: true,
                    },
                )
                .await
                .unwrap();
        }

        let repos = mock.repositories();
        let cache: Arc<dyn CacheCoordinator> = Arc::new(InMemoryCacheCoordinator::new());
        let (collectors, tuning, limits) = test_collectors();
        let pipeline = test_pipeline();

        // 1st tick: fetches once for both users.
        let r1 = run_poller_tick(
            &repos,
            &cache,
            &cipher,
            &collectors,
            &pipeline,
            &tuning,
            &limits,
        )
        .await
        .unwrap();
        assert_eq!(r1.sources_polled, 1);
        assert_eq!(r1.updated, 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // Poll again without force_refresh -> skipped fresh (0 extra HTTP calls).
        let src = mock.sources.all_canonical_sources().pop().unwrap();
        let skipped = poll_canonical_source_once(
            &repos,
            &cache,
            &cipher,
            &collectors,
            &pipeline,
            &tuning,
            &limits,
            &src,
            false,
        )
        .await
        .unwrap();
        assert_eq!(skipped.outcome, FetchOutcome::SkippedFresh);
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // Force refresh -> sends ETag -> 304 NotModified304.
        let not_mod = poll_canonical_source_once(
            &repos,
            &cache,
            &cipher,
            &collectors,
            &pipeline,
            &tuning,
            &limits,
            &src,
            true,
        )
        .await
        .unwrap();
        assert_eq!(not_mod.outcome, FetchOutcome::NotModified304);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn poller_decrypts_user_private_credentials_and_passes_to_collector() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/2/users/by/username/geminidev"))
            .and(header("authorization", "Bearer test-x-private-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "id": "42", "username": "geminidev" }
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/2/users/42/tweets"))
            .and(header("authorization", "Bearer test-x-private-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": "t1",
                    "text": "Gemini X update",
                    "created_at": "2026-11-26T00:00:00Z"
                }]
            })))
            .mount(&server)
            .await;

        let mock = MockRepos::new();
        let cipher = test_cipher();
        let enc = encode_source_config(
            &cipher,
            "@geminidev",
            Some(serde_json::json!({ "bearerToken": "test-x-private-token" })),
        )
        .unwrap();
        let user_id = Uuid::new_v4();
        mock.sources
            .create_or_subscribe(
                user_id,
                SubscribeSourceCmd {
                    source_type: SourceType::XAccount,
                    scope: SourceScope::UserPrivate,
                    canonical_key: "x_account:@geminidev".to_owned(),
                    custom_name: "Gemini X".to_owned(),
                    default_name: "Gemini X".to_owned(),
                    category: "Social".to_owned(),
                    handle_or_url: "@geminidev".to_owned(),
                    config_json_enc: Some(enc),
                    enabled: true,
                },
            )
            .await
            .unwrap();

        let tuning = CollectorTuning {
            max_retry_attempts: NonZeroUsize::new(1).unwrap(),
            initial_backoff: Duration::from_millis(5),
            ..CollectorTuning::default()
        };
        let limits = ResourceLimitsTuning::default();
        let client = shared_http_client(None, &tuning).unwrap();
        let ssrf = SsrfGuard::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()]);
        let ctx = CollectorContext::new(client, ssrf, tuning.clone(), limits.clone())
            .with_x_api_base_url(format!("{}/2", server.uri()));
        let collectors = Arc::new(CollectorRegistry::new(ctx));

        let repos = mock.repositories();
        let cache: Arc<dyn CacheCoordinator> = Arc::new(InMemoryCacheCoordinator::new());
        let report = run_poller_tick(
            &repos,
            &cache,
            &cipher,
            &collectors,
            &test_pipeline(),
            &tuning,
            &limits,
        )
        .await
        .unwrap();
        assert_eq!(report.sources_polled, 1);
        assert_eq!(report.updated, 1);
        assert_eq!(report.items_fetched, 1);
    }

    #[tokio::test]
    async fn poller_applies_exponential_failure_backoff_on_upstream_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/broken.xml"))
            .and(header("accept", "*/*"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let mock = MockRepos::new();
        let cipher = test_cipher();
        let enc =
            encode_source_config(&cipher, &format!("{}/broken.xml", server.uri()), None).unwrap();
        mock.sources
            .create_or_subscribe(
                Uuid::new_v4(),
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "broken-feed".to_owned(),
                    custom_name: "Broken".to_owned(),
                    default_name: "Broken".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: format!("{}/broken.xml", server.uri()),
                    config_json_enc: Some(enc),
                    enabled: true,
                },
            )
            .await
            .unwrap();

        let repos = mock.repositories();
        let cache: Arc<dyn CacheCoordinator> = Arc::new(InMemoryCacheCoordinator::new());
        let (collectors, tuning, limits) = test_collectors();
        let pipeline = test_pipeline();

        let before = Utc::now();
        let report = run_poller_tick(
            &repos,
            &cache,
            &cipher,
            &collectors,
            &pipeline,
            &tuning,
            &limits,
        )
        .await
        .unwrap();
        assert_eq!(report.failed, 1);

        let src = mock.sources.all_canonical_sources().pop().unwrap();
        assert_eq!(src.last_status, Some(FetchOutcome::Failed));
        assert_eq!(src.consecutive_failures, 1);
        let next_poll = src.next_poll_at.unwrap();
        // 1st failure backoff is 1 hour (3600s).
        assert!(next_poll >= before + ChronoDuration::minutes(59));
    }

    #[tokio::test]
    async fn platform_x_bridge_routes_x_account_and_search_while_per_source_token_takes_precedence()
    {
        let bridge = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/twitter/user/GoogleDeepMind"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?><rss version="2.0"><channel><title>@GoogleDeepMind</title><item><guid>x1</guid><title>Gemini bridge post</title><link>https://x.com/GoogleDeepMind/status/1</link></item></channel></rss>"#,
            ))
            .mount(&bridge)
            .await;

        Mock::given(method("GET"))
            .and(path("/twitter/keyword/Gemini%20API"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Search</title><item><guid>xs1</guid><title>Gemini API tweet</title><link>https://x.com/user/status/2</link></item></channel></rss>"#,
            ))
            .mount(&bridge)
            .await;

        let x_api = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/2/users/by/username/CustomHandle"))
            .and(header("authorization", "Bearer per-source-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "id": "99", "username": "CustomHandle" }
            })))
            .mount(&x_api)
            .await;
        Mock::given(method("GET"))
            .and(path("/2/users/99/tweets"))
            .and(header("authorization", "Bearer per-source-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "id": "88", "text": "Direct X API tweet", "created_at": "2026-11-26T00:00:00Z" }]
            })))
            .mount(&x_api)
            .await;

        let mock = MockRepos::new();
        let cipher = test_cipher();
        mock.settings
            .upsert_platform_settings(crate::repo::UpsertPlatformSettingsRecord {
                x_bridge_base_url: Some(bridge.uri()),
                ..crate::repo::UpsertPlatformSettingsRecord::default()
            })
            .await
            .unwrap();

        let tuning = CollectorTuning {
            max_retry_attempts: NonZeroUsize::new(1).unwrap(),
            initial_backoff: Duration::from_millis(5),
            ..CollectorTuning::default()
        };
        let limits = ResourceLimitsTuning::default();
        let client = shared_http_client(None, &tuning).unwrap();
        let ssrf = SsrfGuard::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()]);
        let ctx = CollectorContext::new(client, ssrf, tuning, limits)
            .with_x_api_base_url(format!("{}/2", x_api.uri()));
        let collectors = Arc::new(CollectorRegistry::new(ctx));
        let repos = mock.repositories();

        let acc_desc = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::XAccount,
            scope: SourceScope::UserPrivate,
            canonical_key: "x_account:@GoogleDeepMind".to_owned(),
            name: "GoogleDeepMind".to_owned(),
            handle_or_url: "@GoogleDeepMind".to_owned(),
            category: "Social".to_owned(),
        };
        let acc_res = fetch_with_platform_x_support(
            &repos,
            &cipher,
            &collectors,
            &acc_desc,
            &ConditionalFetchHeaders::default(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(acc_res.items.len(), 1);
        assert_eq!(acc_res.items[0].title, "Gemini bridge post");

        let search_desc = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::XSearch,
            scope: SourceScope::UserPrivate,
            canonical_key: "x_search:Gemini API".to_owned(),
            name: "X Search".to_owned(),
            handle_or_url: "Gemini API".to_owned(),
            category: "Social".to_owned(),
        };
        let search_res = fetch_with_platform_x_support(
            &repos,
            &cipher,
            &collectors,
            &search_desc,
            &ConditionalFetchHeaders::default(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(search_res.items.len(), 1);
        assert_eq!(search_res.items[0].title, "Gemini API tweet");

        // Per-source bearerToken overrides platform bridge
        let custom_desc = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::XAccount,
            scope: SourceScope::UserPrivate,
            canonical_key: "x_account:@CustomHandle".to_owned(),
            name: "CustomHandle".to_owned(),
            handle_or_url: "@CustomHandle".to_owned(),
            category: "Social".to_owned(),
        };
        let per_source_creds = serde_json::json!({ "bearerToken": "per-source-token" });
        let custom_res = fetch_with_platform_x_support(
            &repos,
            &cipher,
            &collectors,
            &custom_desc,
            &ConditionalFetchHeaders::default(),
            Some(&per_source_creds),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(custom_res.items.len(), 1);
        assert_eq!(custom_res.items[0].title, "Direct X API tweet");
    }
}
