//! Layer 2 — Per-user digest composer evaluating timezone-aware daily schedules,
//! matching watchlist rules, running idempotent LLM summarization, offloading
//! multi-format rendering to `spawn_blocking`, persisting digests, dispatching
//! enabled channels, and emitting SSE updates.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use digest_core::cache::CacheCoordinator;
use digest_core::collectors::CollectorRegistry;
use digest_core::constants::{
    DEFAULT_AI_BASE_URL, DEFAULT_AI_MODEL, DEFAULT_AI_PROVIDER_LABEL, DEFAULT_LLM_MAX_TOKENS,
    DEFAULT_LLM_SYSTEM_PROMPT, DEFAULT_LLM_TEMPERATURE, DEFAULT_TIMEZONE,
};
use digest_core::crypto::EnvelopeCipher;
use digest_core::domain::{
    AlertPriority, DailyDigestSynthesisInput, DeliveryStatus, DigestSection, FetchOutcome,
    ItemMatchResult, NotificationChannelType, NotifierConfig, ProcessedItem, RenderInput,
    RunStatus, RunType, SourceDigestInput, SourceSummaryResult, UserAiConfig, WatchlistRuleInput,
};
use digest_core::error::CoreError;
use digest_core::llm::{compute_input_fingerprint, IdempotentLlmRunner};
use digest_core::notifiers::NotifierRegistry;
use digest_core::pipeline::{
    deduplicate_processed_items, match_watchlist, priority_rank, score_item_at,
    sort_processed_items, ItemProcessingPipeline, ScoringWeights,
};
use digest_core::renderer::RendererRegistry;
use digest_core::traits::{
    DigestRenderer, RenderedNotification, SourceDescriptor, SummarizedDigest,
};
use futures::stream::{self, StreamExt};
use uuid::Uuid;

use crate::config::ServerConfig;
use crate::repo::{
    CanonicalItemRecord, CreateDeliveryLogCmd, CreateRunCmd, DailyDigestDetail, DeliveryLogRecord,
    DigestRunView, FinishRunCmd, NewUserItemMatch, NotificationChannelRecord, Repositories,
    SaveDigestCmd, SaveSourceDigestCmd, UserAiConfigRecord, UserScheduleRecord, UserSourceRecord,
    WatchlistRuleRecord,
};
use crate::scheduler::poller::{
    canonical_source_from_user_source, poll_canonical_source_once, SourcePollOutcome,
};
use crate::services::SsePublisher;
use crate::sse_hub::{digest_ready_envelope, run_progress_envelope};

const STAGE_COLLECTING: &str = "collecting";
const STAGE_MATCHING: &str = "matching";
const STAGE_SUMMARIZING: &str = "summarizing";
const STAGE_RENDERING: &str = "rendering";
const STAGE_NOTIFYING: &str = "notifying";
const STAGE_COMPLETED: &str = "completed";

/// Result produced by composing a user's daily digest.
#[derive(Debug, Clone)]
pub struct ComposeOutcome {
    pub run: DigestRunView,
    pub digest: Option<DailyDigestDetail>,
    pub deliveries: Vec<DeliveryLogRecord>,
}

/// Shared dependencies used by the Layer 2 composer and alert scanner.
#[derive(Clone)]
pub struct ComposerDeps {
    pub config: Arc<ServerConfig>,
    pub repos: Repositories,
    pub cipher: Arc<EnvelopeCipher>,
    pub cache: Arc<dyn CacheCoordinator>,
    pub collectors: Arc<CollectorRegistry>,
    pub pipeline: Arc<ItemProcessingPipeline>,
    pub llm_runner: Arc<IdempotentLlmRunner>,
    pub renderer: Arc<RendererRegistry>,
    pub notifiers: Arc<NotifierRegistry>,
    pub sse: Arc<dyn SsePublisher>,
}

/// Runs one scheduled daily composer tick using `Utc::now()`.
pub async fn run_composer_tick(deps: &ComposerDeps) -> Result<usize, CoreError> {
    run_composer_tick_at(deps, Utc::now()).await
}

/// Evaluates all user schedules against `now_utc` and runs daily digests for
/// users whose local date/time has been reached and not yet executed today.
pub async fn run_composer_tick_at(
    deps: &ComposerDeps,
    now_utc: DateTime<Utc>,
) -> Result<usize, CoreError> {
    let schedules = deps.repos.settings.list_due_daily_schedules().await?;
    let mut triggered = 0usize;

    for schedule in schedules {
        let Some(local_date) = scheduled_local_date_if_due(&schedule, now_utc) else {
            continue;
        };

        if let Err(err) = compose_user_digest(
            deps,
            schedule.user_id,
            local_date,
            RunType::ScheduledDaily,
            false,
            true,
            None,
        )
        .await
        {
            tracing::error!(
                user_id = %schedule.user_id,
                digest_date = %local_date,
                error = %err,
                "scheduled daily digest composition failed"
            );
        } else {
            triggered += 1;
        }
    }

    Ok(triggered)
}

/// Determines whether a user's daily schedule is due at `now_utc` and returns
/// their current local `NaiveDate` if it should execute now.
#[must_use]
pub fn scheduled_local_date_if_due(
    schedule: &UserScheduleRecord,
    now_utc: DateTime<Utc>,
) -> Option<NaiveDate> {
    let tz: Tz = schedule
        .timezone
        .trim()
        .parse()
        .unwrap_or_else(|_| DEFAULT_TIMEZONE.parse().unwrap_or(Tz::UTC));
    let local_now = now_utc.with_timezone(&tz);
    let local_date = local_now.date_naive();

    if schedule.last_daily_run_date == Some(local_date) {
        return None;
    }

    let target_time = NaiveTime::parse_from_str(schedule.daily_time.trim(), "%H:%M").ok()?;
    if local_now.time() < target_time {
        return None;
    }

    Some(local_date)
}

/// Composes (and optionally notifies) a user's daily digest for `digest_date`.
pub async fn compose_user_digest(
    deps: &ComposerDeps,
    user_id: Uuid,
    digest_date: NaiveDate,
    run_type: RunType,
    force_refresh: bool,
    notify: bool,
    existing_run_id: Option<Uuid>,
) -> Result<ComposeOutcome, CoreError> {
    let run_record = match existing_run_id {
        Some(run_id) => crate::repo::DigestRunRecord {
            id: run_id,
            user_id,
            run_type,
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            items_fetched: 0,
            items_new: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            error_summary: None,
        },
        None => {
            deps.repos
                .runs
                .create_run(CreateRunCmd {
                    user_id,
                    run_type,
                    started_at: Utc::now(),
                })
                .await?
        }
    };

    let res = compose_inner(
        deps,
        &run_record,
        user_id,
        digest_date,
        run_type,
        force_refresh,
        notify,
    )
    .await;

    match res {
        Ok(outcome) => Ok(outcome),
        Err(err) => {
            let err_msg = err.to_string();
            let _ = deps
                .repos
                .runs
                .finish_run(
                    run_record.id,
                    FinishRunCmd {
                        status: RunStatus::Failed,
                        finished_at: Utc::now(),
                        items_fetched: 0,
                        items_new: 0,
                        prompt_tokens: 0,
                        completion_tokens: 0,
                        error_summary: Some(err_msg.clone()),
                    },
                )
                .await;
            deps.sse.publish(
                user_id,
                run_progress_envelope(
                    run_record.id,
                    run_type,
                    RunStatus::Failed,
                    STAGE_COMPLETED,
                    0,
                    0,
                    Some(&err_msg),
                ),
            );
            Err(err)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn compose_inner(
    deps: &ComposerDeps,
    run_record: &crate::repo::DigestRunRecord,
    user_id: Uuid,
    digest_date: NaiveDate,
    run_type: RunType,
    force_refresh: bool,
    notify: bool,
) -> Result<ComposeOutcome, CoreError> {
    let run_id = run_record.id;
    let schedule = deps.repos.settings.get_schedule(user_id).await?;
    let source_views = deps.repos.sources.list_user_sources(user_id).await?;

    let mut enabled_sources = Vec::new();
    for view in source_views.into_iter().filter(|v| v.enabled) {
        if let Some(rec) = deps.repos.sources.get_user_source(user_id, view.id).await? {
            if rec.enabled {
                enabled_sources.push(rec);
            }
        }
    }

    deps.sse.publish(
        user_id,
        run_progress_envelope(
            run_id,
            run_type,
            RunStatus::Running,
            STAGE_COLLECTING,
            0,
            0,
            None,
        ),
    );

    // Refresh stale enabled sources (using Layer 1 singleflight & freshness cache).
    let concurrency = deps
        .config
        .core
        .resources
        .global_fetch_concurrency
        .get()
        .max(1);
    let poll_outcomes: Vec<SourcePollOutcome> = stream::iter(enabled_sources.iter().cloned())
        .map(|user_src| {
            let repos = deps.repos.clone();
            let cache = Arc::clone(&deps.cache);
            let cipher = Arc::clone(&deps.cipher);
            let collectors = Arc::clone(&deps.collectors);
            let pipeline = Arc::clone(&deps.pipeline);
            let collector_tuning = deps.config.core.collector.clone();
            let limits = deps.config.core.resources.clone();
            async move {
                let canonical = canonical_source_from_user_source(&user_src);
                match poll_canonical_source_once(
                    &repos,
                    &cache,
                    &cipher,
                    &collectors,
                    &pipeline,
                    &collector_tuning,
                    &limits,
                    &canonical,
                    force_refresh,
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

    let items_fetched_usize: usize = poll_outcomes.iter().map(|o| o.items_fetched).sum();
    let items_fetched_i32 = i32::try_from(items_fetched_usize).unwrap_or(i32::MAX);
    let any_fetch_failed = poll_outcomes
        .iter()
        .any(|o| o.outcome == FetchOutcome::Failed);
    let all_fetches_failed = !poll_outcomes.is_empty()
        && poll_outcomes
            .iter()
            .all(|o| o.outcome == FetchOutcome::Failed);

    // Gather recent canonical items across enabled subscribed sources.
    let since = digest_window_start(digest_date, &schedule.timezone, Utc::now());
    let max_items = deps.config.core.llm.max_items_per_digest.max(1);
    let mut source_batches: Vec<(UserSourceRecord, Vec<CanonicalItemRecord>)> =
        Vec::with_capacity(enabled_sources.len());
    for src in &enabled_sources {
        let items = deps
            .repos
            .sources
            .list_recent_source_items(src.canonical_source_id, since, max_items)
            .await?;
        source_batches.push((src.clone(), items));
    }

    deps.sse.publish(
        user_id,
        run_progress_envelope(
            run_id,
            run_type,
            RunStatus::Running,
            STAGE_MATCHING,
            items_fetched_i32,
            0,
            None,
        ),
    );

    let watchlist_records = deps.repos.settings.list_watchlist_rules(user_id).await?;
    let watchlist_inputs: Vec<WatchlistRuleInput> = watchlist_records
        .iter()
        .filter(|r| r.enabled)
        .map(WatchlistRuleRecord::to_core_input)
        .collect();

    // Match & score items in `spawn_blocking` so Tokio NIO worker threads stay free.
    let now = Utc::now();
    let (new_matches, per_source_processed) = tokio::task::spawn_blocking(move || {
        match_and_score_batches(&source_batches, &watchlist_inputs, now)
    })
    .await
    .map_err(|err| CoreError::Internal(format!("watchlist match task failed: {err}")))?;

    if !new_matches.is_empty() {
        deps.repos
            .sources
            .upsert_user_item_matches(user_id, &new_matches)
            .await?;
    }

    let mut combined_items = Vec::new();
    for (_, items) in &per_source_processed {
        combined_items.extend(items.iter().cloned());
    }
    let mut processed_items = deduplicate_processed_items(combined_items);
    sort_processed_items(&mut processed_items);
    if processed_items.len() > max_items {
        processed_items.truncate(max_items);
    }

    let highlighted_items: Vec<ProcessedItem> = processed_items
        .iter()
        .filter(|item| {
            matches!(
                item.match_result.priority,
                AlertPriority::High | AlertPriority::Urgent
            )
        })
        .cloned()
        .collect();

    let items_new_i32 = i32::try_from(processed_items.len()).unwrap_or(i32::MAX);

    // Handle 0 items cleanly without spurious emails on scheduled runs when `!send_when_empty`.
    if processed_items.is_empty()
        && run_type == RunType::ScheduledDaily
        && !schedule.send_when_empty
    {
        let finished_at = Utc::now();
        let final_status = if all_fetches_failed {
            RunStatus::Failed
        } else if any_fetch_failed {
            RunStatus::Partial
        } else {
            RunStatus::Succeeded
        };
        let error_summary = if any_fetch_failed {
            Some("one or more sources failed during scheduled poll".to_owned())
        } else {
            None
        };
        deps.repos
            .runs
            .finish_run(
                run_id,
                FinishRunCmd {
                    status: final_status,
                    finished_at,
                    items_fetched: items_fetched_i32,
                    items_new: 0,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    error_summary: error_summary.clone(),
                },
            )
            .await?;
        deps.repos
            .settings
            .mark_daily_run_date(user_id, digest_date)
            .await?;

        deps.sse.publish(
            user_id,
            run_progress_envelope(
                run_id,
                run_type,
                final_status,
                STAGE_COMPLETED,
                items_fetched_i32,
                0,
                Some("no new items collected"),
            ),
        );

        let run = DigestRunView {
            id: run_id,
            run_type,
            status: final_status,
            started_at: run_record.started_at,
            finished_at: Some(finished_at),
            items_fetched: items_fetched_i32,
            items_new: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            error_summary,
            deliveries: Vec::new(),
        };
        return Ok(ComposeOutcome {
            run,
            digest: None,
            deliveries: Vec::new(),
        });
    }

    // Resolve and decrypt user AI config (`spawn_blocking` if encrypted key present).
    let ai_record = deps.repos.settings.get_ai_config(user_id).await?;
    let ai_config = resolve_user_ai_config_blocking(
        Arc::clone(&deps.cipher),
        ai_record,
        deps.config.core.llm.default_max_tokens,
    )
    .await;

    let daily_fp = compute_input_fingerprint(digest_date, &ai_config, &processed_items);

    // DB-level fingerprint idempotency check: 0 duplicate LLM tokens when inputs unchanged.
    if !force_refresh {
        if let Some(existing_digest) = deps
            .repos
            .digests
            .find_daily_digest_by_fingerprint(user_id, digest_date, &daily_fp)
            .await?
        {
            let should_notify = notify && (!processed_items.is_empty() || schedule.send_when_empty);
            let deliveries = if should_notify {
                let rendered = render_existing_digest_blocking(
                    Arc::clone(&deps.renderer),
                    &existing_digest,
                    &highlighted_items,
                    &processed_items,
                )
                .await?;
                dispatch_daily_notifications(
                    deps,
                    user_id,
                    run_id,
                    existing_digest.id,
                    &schedule,
                    &rendered,
                )
                .await
            } else {
                Vec::new()
            };

            let any_delivery_failed = deliveries
                .iter()
                .any(|d| d.status == DeliveryStatus::Failed);
            let final_status = compute_final_run_status(
                all_fetches_failed && processed_items.is_empty(),
                any_fetch_failed,
                any_delivery_failed,
            );
            let finished_at = Utc::now();

            deps.repos
                .runs
                .finish_run(
                    run_id,
                    FinishRunCmd {
                        status: final_status,
                        finished_at,
                        items_fetched: items_fetched_i32,
                        items_new: items_new_i32,
                        prompt_tokens: 0,
                        completion_tokens: 0,
                        error_summary: None,
                    },
                )
                .await?;

            if run_type == RunType::ScheduledDaily {
                deps.repos
                    .settings
                    .mark_daily_run_date(user_id, digest_date)
                    .await?;
            }

            deps.sse.publish(
                user_id,
                run_progress_envelope(
                    run_id,
                    run_type,
                    final_status,
                    STAGE_COMPLETED,
                    items_fetched_i32,
                    items_new_i32,
                    Some("reused idempotent digest"),
                ),
            );
            deps.sse
                .publish(user_id, digest_ready_envelope(&existing_digest));

            let run = DigestRunView {
                id: run_id,
                run_type,
                status: final_status,
                started_at: run_record.started_at,
                finished_at: Some(finished_at),
                items_fetched: items_fetched_i32,
                items_new: items_new_i32,
                prompt_tokens: 0,
                completion_tokens: 0,
                error_summary: None,
                deliveries: deliveries.clone(),
            };
            return Ok(ComposeOutcome {
                run,
                digest: Some(existing_digest),
                deliveries,
            });
        }
    }

    deps.sse.publish(
        user_id,
        run_progress_envelope(
            run_id,
            run_type,
            RunStatus::Running,
            STAGE_SUMMARIZING,
            items_fetched_i32,
            items_new_i32,
            None,
        ),
    );

    let (
        effective_fp,
        summarized_digest,
        source_summaries,
        sections,
        prompt_tokens_u32,
        completion_tokens_u32,
    ) = if processed_items.is_empty() {
        (
            daily_fp,
            SummarizedDigest {
                title: format!("Gemini Daily Digest — {digest_date}"),
                overview_markdown: format!("No new items collected for {digest_date}."),
                prompt_tokens: 0,
                completion_tokens: 0,
                model_used: ai_config.model.clone(),
                degraded_fallback: false,
            },
            Vec::new(),
            Vec::new(),
            0,
            0,
        )
    } else {
        summarize_per_source_and_daily(
            deps,
            &ai_config,
            digest_date,
            &per_source_processed,
            &highlighted_items,
            &processed_items,
            force_refresh,
            concurrency,
        )
        .await?
    };

    deps.sse.publish(
        user_id,
        run_progress_envelope(
            run_id,
            run_type,
            RunStatus::Running,
            STAGE_RENDERING,
            items_fetched_i32,
            items_new_i32,
            None,
        ),
    );

    let highest_priority = processed_items
        .iter()
        .map(|item| item.match_result.priority)
        .max_by_key(|priority| priority_rank(*priority))
        .unwrap_or(AlertPriority::Normal);

    let render_input = RenderInput {
        digest_date,
        digest: summarized_digest.clone(),
        sections: sections.clone(),
        highlighted_items: highlighted_items.clone(),
        all_items: processed_items.clone(),
        priority: highest_priority,
    };

    let renderer = Arc::clone(&deps.renderer);
    let rendered = tokio::task::spawn_blocking(move || renderer.render_structured(&render_input))
        .await
        .map_err(|err| CoreError::Internal(format!("digest render task failed: {err}")))??;

    let source_digests: Vec<SaveSourceDigestCmd> = source_summaries
        .into_iter()
        .map(|summary| SaveSourceDigestCmd {
            canonical_source_id: summary.source_id,
            digest_date: summary.digest_date,
            input_fingerprint: summary.input_fingerprint,
            summary_markdown: summary.summary_markdown,
            key_points: summary.key_points,
            item_count: i32::try_from(summary.item_count).unwrap_or(i32::MAX),
        })
        .collect();

    let saved_digest = deps
        .repos
        .digests
        .save_daily_and_source_digests(SaveDigestCmd {
            user_id,
            digest_date,
            input_fingerprint: effective_fp,
            title: summarized_digest.title,
            overview_markdown: summarized_digest.overview_markdown,
            overview_html: rendered.html_body.clone(),
            sections,
            item_count: items_new_i32,
            highlighted_count: i32::try_from(highlighted_items.len()).unwrap_or(i32::MAX),
            prompt_tokens: i32::try_from(prompt_tokens_u32).unwrap_or(i32::MAX),
            completion_tokens: i32::try_from(completion_tokens_u32).unwrap_or(i32::MAX),
            model_used: summarized_digest.model_used,
            degraded_fallback: summarized_digest.degraded_fallback,
            source_digests,
        })
        .await?;

    let should_notify = notify && (!processed_items.is_empty() || schedule.send_when_empty);
    let deliveries = if should_notify {
        deps.sse.publish(
            user_id,
            run_progress_envelope(
                run_id,
                run_type,
                RunStatus::Running,
                STAGE_NOTIFYING,
                items_fetched_i32,
                items_new_i32,
                None,
            ),
        );
        dispatch_daily_notifications(deps, user_id, run_id, saved_digest.id, &schedule, &rendered)
            .await
    } else {
        Vec::new()
    };

    let any_delivery_failed = deliveries
        .iter()
        .any(|d| d.status == DeliveryStatus::Failed);
    let final_status = compute_final_run_status(
        all_fetches_failed && processed_items.is_empty(),
        any_fetch_failed,
        any_delivery_failed,
    );
    let finished_at = Utc::now();
    let error_summary = build_run_error_summary(any_fetch_failed, any_delivery_failed);

    deps.repos
        .runs
        .finish_run(
            run_id,
            FinishRunCmd {
                status: final_status,
                finished_at,
                items_fetched: items_fetched_i32,
                items_new: items_new_i32,
                prompt_tokens: i32::try_from(prompt_tokens_u32).unwrap_or(i32::MAX),
                completion_tokens: i32::try_from(completion_tokens_u32).unwrap_or(i32::MAX),
                error_summary: error_summary.clone(),
            },
        )
        .await?;

    if run_type == RunType::ScheduledDaily {
        deps.repos
            .settings
            .mark_daily_run_date(user_id, digest_date)
            .await?;
    }

    deps.sse.publish(
        user_id,
        run_progress_envelope(
            run_id,
            run_type,
            final_status,
            STAGE_COMPLETED,
            items_fetched_i32,
            items_new_i32,
            None,
        ),
    );
    deps.sse
        .publish(user_id, digest_ready_envelope(&saved_digest));

    let run = DigestRunView {
        id: run_id,
        run_type,
        status: final_status,
        started_at: run_record.started_at,
        finished_at: Some(finished_at),
        items_fetched: items_fetched_i32,
        items_new: items_new_i32,
        prompt_tokens: i32::try_from(prompt_tokens_u32).unwrap_or(i32::MAX),
        completion_tokens: i32::try_from(completion_tokens_u32).unwrap_or(i32::MAX),
        error_summary,
        deliveries: deliveries.clone(),
    };

    Ok(ComposeOutcome {
        run,
        digest: Some(saved_digest),
        deliveries,
    })
}

#[allow(clippy::too_many_arguments)]
async fn summarize_per_source_and_daily(
    deps: &ComposerDeps,
    ai_config: &UserAiConfig,
    digest_date: NaiveDate,
    per_source_processed: &[(UserSourceRecord, Vec<ProcessedItem>)],
    highlighted_items: &[ProcessedItem],
    processed_items: &[ProcessedItem],
    force_refresh: bool,
    concurrency: usize,
) -> Result<
    (
        String,
        SummarizedDigest,
        Vec<SourceSummaryResult>,
        Vec<DigestSection>,
        u32,
        u32,
    ),
    CoreError,
> {
    let non_empty_sources: Vec<(usize, SourceDescriptor, Vec<ProcessedItem>)> =
        per_source_processed
            .iter()
            .filter(|(_, items)| !items.is_empty())
            .enumerate()
            .map(|(idx, (src, items))| {
                let descriptor = SourceDescriptor {
                    id: src.canonical_source_id,
                    source_type: src.source_type,
                    scope: src.scope,
                    canonical_key: src.canonical_key.clone(),
                    name: src.custom_name.clone(),
                    handle_or_url: String::new(),
                    category: src.category.clone(),
                };
                (idx, descriptor, items.clone())
            })
            .collect();

    let mut indexed_summaries: Vec<(usize, Result<SourceSummaryResult, CoreError>)> =
        stream::iter(non_empty_sources)
            .map(|(idx, source, items)| {
                let llm_runner = Arc::clone(&deps.llm_runner);
                let ai_config = ai_config.clone();
                async move {
                    let input = SourceDigestInput {
                        source,
                        digest_date,
                        items,
                    };
                    let res = llm_runner
                        .summarize_source_idempotent(&ai_config, &input, force_refresh)
                        .await;
                    (idx, res)
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

    indexed_summaries.sort_by_key(|(idx, _)| *idx);

    let mut source_summaries = Vec::with_capacity(indexed_summaries.len());
    for (_, res) in indexed_summaries {
        source_summaries.push(res?);
    }

    let daily_input = DailyDigestSynthesisInput {
        digest_date,
        source_summaries: source_summaries.clone(),
        highlighted_items: highlighted_items.to_vec(),
        all_items: processed_items.to_vec(),
    };

    let daily_outcome = deps
        .llm_runner
        .summarize_daily_with_options(ai_config, &daily_input, force_refresh)
        .await?;

    let mut items_by_source: HashMap<Uuid, Vec<ProcessedItem>> = HashMap::new();
    for (src, items) in per_source_processed {
        if !items.is_empty() {
            items_by_source.insert(src.canonical_source_id, items.clone());
        }
    }

    let sections: Vec<DigestSection> = source_summaries
        .iter()
        .map(|summary| DigestSection {
            category: summary.category.clone(),
            source_id: Some(summary.source_id),
            source_name: summary.source_name.clone(),
            summary_markdown: summary.summary_markdown.clone(),
            key_points: summary.key_points.clone(),
            items: items_by_source
                .get(&summary.source_id)
                .cloned()
                .unwrap_or_default(),
        })
        .collect();

    let (prompt_tokens, completion_tokens) = if daily_outcome.cache_hit {
        (0, 0)
    } else {
        let src_prompt: u32 = source_summaries.iter().map(|s| s.prompt_tokens).sum();
        let src_completion: u32 = source_summaries.iter().map(|s| s.completion_tokens).sum();
        (
            src_prompt.saturating_add(daily_outcome.digest.prompt_tokens),
            src_completion.saturating_add(daily_outcome.digest.completion_tokens),
        )
    };

    Ok((
        daily_outcome.input_fingerprint,
        daily_outcome.digest,
        source_summaries,
        sections,
        prompt_tokens,
        completion_tokens,
    ))
}

/// Computes the inclusive lookback timestamp for gathering items belonging to `digest_date`.
#[must_use]
pub fn digest_window_start(
    digest_date: NaiveDate,
    timezone: &str,
    now_utc: DateTime<Utc>,
) -> DateTime<Utc> {
    let tz: Tz = timezone
        .trim()
        .parse()
        .unwrap_or_else(|_| DEFAULT_TIMEZONE.parse().unwrap_or(Tz::UTC));
    let anchor_utc = digest_date
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| tz.from_local_datetime(&naive).earliest())
        .map(|ldt| ldt.with_timezone(&Utc))
        .unwrap_or(now_utc);

    (anchor_utc - ChronoDuration::hours(24)).min(now_utc - ChronoDuration::hours(24))
}

/// Synchronously matches and scores canonical item batches against enabled watchlist rules.
#[must_use]
pub fn match_and_score_batches(
    batches: &[(UserSourceRecord, Vec<CanonicalItemRecord>)],
    rules: &[WatchlistRuleInput],
    now: DateTime<Utc>,
) -> (
    Vec<NewUserItemMatch>,
    Vec<(UserSourceRecord, Vec<ProcessedItem>)>,
) {
    let weights = ScoringWeights::default();
    let mut matches_out = Vec::new();
    let mut seen_match_ids = HashSet::new();
    let mut per_source = Vec::with_capacity(batches.len());

    for (source, items) in batches {
        let mut processed_for_source = Vec::with_capacity(items.len());
        for item in items {
            let wl = match_watchlist(
                &item.title,
                item.summary_raw.as_deref(),
                item.author.as_deref(),
                rules,
            );
            let importance_score = score_item_at(
                item.published_at,
                item.summary_raw.as_deref(),
                &wl.matched_keywords,
                wl.priority,
                now,
                &weights,
            );

            if (!wl.matched_keywords.is_empty() || wl.notify_immediately)
                && seen_match_ids.insert(item.id)
            {
                matches_out.push(NewUserItemMatch {
                    item_id: item.id,
                    matched_keywords: wl.matched_keywords.clone(),
                    priority: wl.priority,
                    importance_score,
                });
            }

            processed_for_source.push(ProcessedItem {
                external_id: item.external_id.clone(),
                canonical_url: item.canonical_url.clone(),
                content_hash: item.content_hash.clone(),
                title: item.title.clone(),
                author: item.author.clone(),
                summary: item.summary_raw.clone(),
                published_at: item.published_at,
                source_id: Some(source.canonical_source_id),
                source_name: Some(source.custom_name.clone()),
                category: Some(source.category.clone()),
                match_result: ItemMatchResult {
                    matched_keywords: wl.matched_keywords,
                    priority: wl.priority,
                    importance_score,
                    notify_immediately: wl.notify_immediately,
                },
            });
        }
        sort_processed_items(&mut processed_for_source);
        per_source.push((source.clone(), processed_for_source));
    }

    (matches_out, per_source)
}

/// Resolves and decrypts a user's `UserAiConfig`, offloading decryption to `spawn_blocking`.
pub async fn resolve_user_ai_config_blocking(
    cipher: Arc<EnvelopeCipher>,
    record: Option<UserAiConfigRecord>,
    default_max_tokens: u32,
) -> UserAiConfig {
    let Some(rec) = record else {
        return UserAiConfig {
            provider_label: DEFAULT_AI_PROVIDER_LABEL.to_owned(),
            base_url: DEFAULT_AI_BASE_URL.to_owned(),
            api_key: None,
            model: DEFAULT_AI_MODEL.to_owned(),
            system_prompt: DEFAULT_LLM_SYSTEM_PROMPT.to_owned(),
            temperature: DEFAULT_LLM_TEMPERATURE,
            max_tokens: DEFAULT_LLM_MAX_TOKENS,
        };
    };

    let user_id = rec.user_id;
    let api_key = match rec.api_key_enc {
        Some(enc) if !enc.trim().is_empty() => {
            match tokio::task::spawn_blocking(move || cipher.decrypt(&enc)).await {
                Ok(Ok(plain)) => Some(plain),
                Ok(Err(err)) => {
                    tracing::warn!(
                        user_id = %user_id,
                        error = %err,
                        "failed to decrypt user AI key; falling back"
                    );
                    None
                }
                Err(err) => {
                    tracing::warn!(
                        user_id = %user_id,
                        error = %err,
                        "AI key decryption task panicked; falling back"
                    );
                    None
                }
            }
        }
        _ => None,
    };

    UserAiConfig {
        provider_label: rec.provider_label,
        base_url: rec.base_url,
        api_key,
        model: rec.model,
        system_prompt: rec.system_prompt,
        temperature: rec.temperature,
        max_tokens: u32::try_from(rec.max_tokens.max(1)).unwrap_or(default_max_tokens),
    }
}

/// Decrypts a batch of notification channels inside `spawn_blocking`.
pub async fn decrypt_channels_blocking(
    cipher: Arc<EnvelopeCipher>,
    channels: Vec<NotificationChannelRecord>,
) -> Vec<(NotificationChannelRecord, Result<NotifierConfig, CoreError>)> {
    if channels.is_empty() {
        return Vec::new();
    }

    tokio::task::spawn_blocking(move || {
        channels
            .into_iter()
            .map(|ch| {
                let parsed = cipher.decrypt(&ch.config_enc).and_then(|plain| {
                    serde_json::from_str::<serde_json::Value>(&plain)
                        .map(|config_json| NotifierConfig {
                            channel_id: Some(ch.id),
                            channel_type: ch.channel_type,
                            name: ch.name.clone(),
                            config_json,
                        })
                        .map_err(|err| {
                            CoreError::Crypto(format!("invalid decrypted channel JSON: {err}"))
                        })
                });
                (ch, parsed)
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

async fn render_existing_digest_blocking(
    renderer: Arc<RendererRegistry>,
    digest: &DailyDigestDetail,
    highlighted_items: &[ProcessedItem],
    all_items: &[ProcessedItem],
) -> Result<RenderedNotification, CoreError> {
    let highest_priority = all_items
        .iter()
        .map(|item| item.match_result.priority)
        .max_by_key(|priority| priority_rank(*priority))
        .unwrap_or(AlertPriority::Normal);

    let input = RenderInput {
        digest_date: digest.digest_date,
        digest: SummarizedDigest {
            title: digest.title.clone(),
            overview_markdown: digest.overview_markdown.clone(),
            prompt_tokens: 0,
            completion_tokens: 0,
            model_used: digest.model_used.clone(),
            degraded_fallback: digest.degraded_fallback,
        },
        sections: digest.sections.clone(),
        highlighted_items: highlighted_items.to_vec(),
        all_items: all_items.to_vec(),
        priority: highest_priority,
    };

    tokio::task::spawn_blocking(move || renderer.render_structured(&input))
        .await
        .map_err(|err| CoreError::Internal(format!("digest render task failed: {err}")))?
}

async fn dispatch_daily_notifications(
    deps: &ComposerDeps,
    user_id: Uuid,
    run_id: Uuid,
    digest_id: Uuid,
    schedule: &UserScheduleRecord,
    rendered: &RenderedNotification,
) -> Vec<DeliveryLogRecord> {
    let channels = match deps.repos.settings.list_channels(user_id).await {
        Ok(list) => list,
        Err(err) => {
            tracing::warn!(user_id = %user_id, error = %err, "failed to load notification channels");
            return Vec::new();
        }
    };

    let selected: Vec<NotificationChannelRecord> = channels
        .into_iter()
        .filter(|ch| {
            ch.enabled
                && ch.trigger_on_daily
                && (ch.channel_type != NotificationChannelType::Smtp || schedule.email_enabled)
        })
        .collect();

    dispatch_selected_channels(
        &deps.repos,
        Arc::clone(&deps.cipher),
        Arc::clone(&deps.notifiers),
        user_id,
        run_id,
        Some(digest_id),
        selected,
        rendered,
        deps.config
            .core
            .resources
            .global_fetch_concurrency
            .get()
            .max(1),
    )
    .await
}

/// Decrypts and dispatches a set of pre-filtered notification channels, recording `delivery_logs`.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_selected_channels(
    repos: &Repositories,
    cipher: Arc<EnvelopeCipher>,
    notifiers: Arc<NotifierRegistry>,
    user_id: Uuid,
    run_id: Uuid,
    digest_id: Option<Uuid>,
    selected: Vec<NotificationChannelRecord>,
    rendered: &RenderedNotification,
    concurrency: usize,
) -> Vec<DeliveryLogRecord> {
    if selected.is_empty() {
        return Vec::new();
    }

    let decrypted = decrypt_channels_blocking(cipher, selected).await;
    let indexed: Vec<(
        usize,
        NotificationChannelRecord,
        Result<NotifierConfig, CoreError>,
    )> = decrypted
        .into_iter()
        .enumerate()
        .map(|(i, (c, r))| (i, c, r))
        .collect();

    let mut sent_results: Vec<(usize, CreateDeliveryLogCmd)> = stream::iter(indexed)
        .map(|(idx, channel_rec, cfg_res)| {
            let notifiers = Arc::clone(&notifiers);
            let rendered = rendered.clone();
            async move {
                let (status, error_message) = match cfg_res {
                    Ok(notifier_cfg) => match notifiers.send(&notifier_cfg, &rendered).await {
                        Ok(()) => (DeliveryStatus::Sent, None),
                        Err(err) => (DeliveryStatus::Failed, Some(err.to_string())),
                    },
                    Err(err) => (DeliveryStatus::Failed, Some(err.to_string())),
                };

                let cmd = CreateDeliveryLogCmd {
                    run_id,
                    user_id,
                    digest_id,
                    channel_id: Some(channel_rec.id),
                    channel_type: channel_rec.channel_type,
                    status,
                    error_message,
                    sent_at: Utc::now(),
                };
                (idx, cmd)
            }
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;

    sent_results.sort_by_key(|(idx, _)| *idx);

    let mut records = Vec::with_capacity(sent_results.len());
    for (_, cmd) in sent_results {
        match repos.runs.record_delivery(cmd).await {
            Ok(rec) => records.push(rec),
            Err(err) => {
                tracing::warn!(user_id = %user_id, error = %err, "failed to record delivery log");
            }
        }
    }
    records
}

fn compute_final_run_status(
    all_sources_failed_with_empty_items: bool,
    any_fetch_failed: bool,
    any_delivery_failed: bool,
) -> RunStatus {
    if all_sources_failed_with_empty_items {
        RunStatus::Failed
    } else if any_fetch_failed || any_delivery_failed {
        RunStatus::Partial
    } else {
        RunStatus::Succeeded
    }
}

fn build_run_error_summary(any_fetch_failed: bool, any_delivery_failed: bool) -> Option<String> {
    match (any_fetch_failed, any_delivery_failed) {
        (true, true) => Some("one or more sources and notification deliveries failed".to_owned()),
        (true, false) => Some("one or more sources failed to refresh".to_owned()),
        (false, true) => Some("one or more notification deliveries failed".to_owned()),
        (false, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use chrono::TimeZone;
    use digest_core::cache::InMemoryCacheCoordinator;
    use digest_core::collectors::CollectorContext;
    use digest_core::config::{CollectorTuning, LlmTuning, NotifierTuning};
    use digest_core::domain::{SourceScope, SourceType};
    use digest_core::http::shared_http_client;
    use digest_core::llm::OpenAiCompatProvider;
    use digest_core::ssrf::SsrfGuard;
    use digest_core::traits::StructuredLlmProvider;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;
    use crate::repo::{
        encode_source_config, CreateChannelRecord, SubscribeSourceCmd, UpdateScheduleCmd,
        UpsertAiConfigRecord, UpsertWatchlistRuleCmd,
    };
    use crate::scheduler::test_support::MockRepos;
    use crate::sse_hub::SseHub;

    fn build_deps(
        mock: &MockRepos,
        server: &MockServer,
        llm_hits: Arc<AtomicUsize>,
    ) -> ComposerDeps {
        let mut cfg = ServerConfig::default();
        cfg.core.collector = CollectorTuning {
            max_retry_attempts: NonZeroUsize::new(1).unwrap(),
            initial_backoff: Duration::from_millis(5),
            ..CollectorTuning::default()
        };
        cfg.core.llm = LlmTuning {
            request_timeout: Duration::from_secs(5),
            ..LlmTuning::default()
        };
        cfg.core.ssrf_allowlist_hosts = vec!["127.0.0.1".to_owned(), "localhost".to_owned()];

        let client = shared_http_client(None, &cfg.core.collector).unwrap();
        let ssrf = SsrfGuard::from_config(&cfg.core);
        let collectors = Arc::new(CollectorRegistry::new(CollectorContext::new(
            client.clone(),
            ssrf.clone(),
            cfg.core.collector.clone(),
            cfg.core.resources.clone(),
        )));
        let cache: Arc<dyn CacheCoordinator> = Arc::new(InMemoryCacheCoordinator::new());

        struct CountingLlm {
            inner: OpenAiCompatProvider,
            hits: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl digest_core::traits::LlmProvider for CountingLlm {
            async fn summarize(
                &self,
                system_prompt: &str,
                items: &[digest_core::traits::RawCollectedItem],
            ) -> Result<SummarizedDigest, CoreError> {
                self.inner.summarize(system_prompt, items).await
            }
        }

        #[async_trait::async_trait]
        impl StructuredLlmProvider for CountingLlm {
            async fn summarize_source(
                &self,
                config: &UserAiConfig,
                input: &SourceDigestInput,
            ) -> Result<SourceSummaryResult, CoreError> {
                self.hits.fetch_add(1, Ordering::SeqCst);
                self.inner.summarize_source(config, input).await
            }

            async fn synthesize_daily(
                &self,
                config: &UserAiConfig,
                input: &DailyDigestSynthesisInput,
            ) -> Result<SummarizedDigest, CoreError> {
                self.hits.fetch_add(1, Ordering::SeqCst);
                self.inner.synthesize_daily(config, input).await
            }

            async fn test_connection(&self, config: &UserAiConfig) -> Result<(), CoreError> {
                self.inner.test_connection(config).await
            }
        }

        let provider: Arc<dyn StructuredLlmProvider> = Arc::new(CountingLlm {
            inner: OpenAiCompatProvider::with_tuning(
                client.clone(),
                ssrf.clone(),
                cfg.core.llm.clone(),
                cfg.core.resources.max_feed_bytes.get(),
            ),
            hits: llm_hits,
        });
        let llm_runner = Arc::new(IdempotentLlmRunner::new(Arc::clone(&cache), provider));
        let notifiers = Arc::new(NotifierRegistry::with_defaults(
            client,
            ssrf,
            NotifierTuning::default(),
        ));

        let _ = server.uri();
        ComposerDeps {
            config: Arc::new(cfg),
            repos: mock.repositories(),
            cipher: Arc::new(EnvelopeCipher::from_bytes([0x22u8; 32])),
            cache,
            collectors,
            pipeline: Arc::new(ItemProcessingPipeline::new()),
            llm_runner,
            renderer: Arc::new(RendererRegistry::new()),
            notifiers,
            sse: Arc::new(SseHub::new()),
        }
    }

    #[test]
    fn scheduled_local_date_respects_timezone_and_daily_time() {
        let user_id = Uuid::new_v4();
        let schedule = UserScheduleRecord {
            user_id,
            timezone: "Asia/Shanghai".to_owned(),
            daily_time: "08:30".to_owned(),
            email_enabled: true,
            send_when_empty: false,
            alert_scan_enabled: true,
            alert_interval_minutes: 60,
            last_daily_run_date: None,
            last_alert_scan_at: None,
        };

        // 2026-11-26 00:29 UTC == 08:29 Asia/Shanghai -> not due yet.
        let before = Utc.with_ymd_and_hms(2026, 11, 26, 0, 29, 0).unwrap();
        assert_eq!(scheduled_local_date_if_due(&schedule, before), None);

        // 2026-11-26 00:30 UTC == 08:30 Asia/Shanghai -> due for 2026-11-26.
        let exact = Utc.with_ymd_and_hms(2026, 11, 26, 0, 30, 0).unwrap();
        assert_eq!(
            scheduled_local_date_if_due(&schedule, exact),
            Some(NaiveDate::from_ymd_opt(2026, 11, 26).unwrap())
        );

        // Once marked run for 2026-11-26, not due again on same local date.
        let mut ran = schedule;
        ran.last_daily_run_date = NaiveDate::from_ymd_opt(2026, 11, 26);
        assert_eq!(scheduled_local_date_if_due(&ran, exact), None);
    }

    #[tokio::test]
    async fn composer_respects_send_when_empty_false_and_marks_date() {
        let server = MockServer::start().await;
        let mock = MockRepos::new();
        let llm_hits = Arc::new(AtomicUsize::new(0));
        let deps = build_deps(&mock, &server, Arc::clone(&llm_hits));

        let user_id = Uuid::new_v4();
        deps.repos
            .settings
            .upsert_schedule(
                user_id,
                UpdateScheduleCmd {
                    timezone: "UTC".to_owned(),
                    daily_time: "08:00".to_owned(),
                    email_enabled: true,
                    send_when_empty: false,
                    alert_scan_enabled: false,
                    alert_interval_minutes: 60,
                },
            )
            .await
            .unwrap();

        let webhook_enc = deps
            .cipher
            .encrypt(&serde_json::json!({ "url": format!("{}/hook", server.uri()) }).to_string())
            .unwrap();
        deps.repos
            .settings
            .create_channel(
                user_id,
                CreateChannelRecord {
                    channel_type: NotificationChannelType::Webhook,
                    name: "Hook".to_owned(),
                    enabled: true,
                    config_enc: webhook_enc,
                    config_hint: "https://example.com".to_owned(),
                    trigger_on_daily: true,
                    trigger_on_alert: true,
                },
            )
            .await
            .unwrap();

        let now = Utc.with_ymd_and_hms(2026, 11, 26, 8, 5, 0).unwrap();
        let ran = run_composer_tick_at(&deps, now).await.unwrap();
        assert_eq!(ran, 1);
        assert_eq!(llm_hits.load(Ordering::SeqCst), 0);

        let updated_sched = deps.repos.settings.get_schedule(user_id).await.unwrap();
        assert_eq!(
            updated_sched.last_daily_run_date,
            NaiveDate::from_ymd_opt(2026, 11, 26)
        );
        // No digest saved and no spurious delivery logged.
        assert!(deps
            .repos
            .digests
            .get_daily_digest_by_date(user_id, NaiveDate::from_ymd_opt(2026, 11, 26).unwrap())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn composer_is_idempotent_on_fingerprint_and_emits_sse_and_webhook() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rss.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Official</title><item><guid>g1</guid><title>Gemini 2.5 Pro Release</title><link>https://example.com/gemini-2-5</link><description>Major launch.</description></item></channel></rss>"#,
            ))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |_req: &Request| {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "model": "deepseek-chat",
                    "choices": [{ "message": { "content": "Concise Gemini summary." } }],
                    "usage": { "prompt_tokens": 12, "completion_tokens": 8 }
                }))
            })
            .mount(&server)
            .await;

        let webhook_hits = Arc::new(AtomicUsize::new(0));
        let webhook_hits_clone = Arc::clone(&webhook_hits);
        Mock::given(method("POST"))
            .and(path("/notify"))
            .respond_with(move |_req: &Request| {
                webhook_hits_clone.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
            })
            .mount(&server)
            .await;

        let mock = MockRepos::new();
        let llm_hits = Arc::new(AtomicUsize::new(0));
        let deps = build_deps(&mock, &server, Arc::clone(&llm_hits));

        let user_id = Uuid::new_v4();
        let mut sse_rx = deps.sse.subscribe(user_id);

        let src_enc =
            encode_source_config(&deps.cipher, &format!("{}/rss.xml", server.uri()), None).unwrap();
        deps.repos
            .sources
            .create_or_subscribe(
                user_id,
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "composer-rss".to_owned(),
                    custom_name: "Official Blog".to_owned(),
                    default_name: "Official Blog".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: format!("{}/rss.xml", server.uri()),
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
                    name: "Gemini".to_owned(),
                    keywords: vec!["Gemini 2.5".to_owned()],
                    exclude_keywords: Vec::new(),
                    priority: AlertPriority::Urgent,
                    notify_immediately: true,
                    enabled: true,
                },
            )
            .await
            .unwrap();

        let ai_key_enc = deps.cipher.encrypt("test-secret-ai-key").unwrap();
        deps.repos
            .settings
            .upsert_ai_config(
                user_id,
                UpsertAiConfigRecord {
                    provider_label: "DeepSeek".to_owned(),
                    base_url: server.uri(),
                    api_key_enc: Some(ai_key_enc),
                    api_key_hint: Some("test****-key".to_owned()),
                    model: "deepseek-chat".to_owned(),
                    system_prompt: "Summarize.".to_owned(),
                    temperature: 0.2,
                    max_tokens: 1024,
                },
            )
            .await
            .unwrap();

        let hook_enc = deps
            .cipher
            .encrypt(&serde_json::json!({ "url": format!("{}/notify", server.uri()) }).to_string())
            .unwrap();
        deps.repos
            .settings
            .create_channel(
                user_id,
                CreateChannelRecord {
                    channel_type: NotificationChannelType::Webhook,
                    name: "Hook".to_owned(),
                    enabled: true,
                    config_enc: hook_enc,
                    config_hint: "https://example.com".to_owned(),
                    trigger_on_daily: true,
                    trigger_on_alert: true,
                },
            )
            .await
            .unwrap();

        let date = Utc::now().date_naive();
        let first = compose_user_digest(
            &deps,
            user_id,
            date,
            RunType::ManualDaily,
            false,
            true,
            None,
        )
        .await
        .unwrap();
        assert_eq!(first.run.status, RunStatus::Succeeded);
        assert!(first.run.prompt_tokens > 0);
        assert_eq!(first.deliveries.len(), 1);
        assert_eq!(first.deliveries[0].status, DeliveryStatus::Sent);
        assert_eq!(webhook_hits.load(Ordering::SeqCst), 1);
        let first_llm_calls = llm_hits.load(Ordering::SeqCst);
        assert_eq!(first_llm_calls, 2); // 1 source summary + 1 daily synthesis

        // Repeat run with unchanged inputs (`force_refresh = false`) -> 0 LLM calls/tokens!
        let second = compose_user_digest(
            &deps,
            user_id,
            date,
            RunType::ManualDaily,
            false,
            false,
            None,
        )
        .await
        .unwrap();
        assert_eq!(second.run.status, RunStatus::Succeeded);
        assert_eq!(second.run.prompt_tokens, 0);
        assert_eq!(second.run.completion_tokens, 0);
        assert_eq!(llm_hits.load(Ordering::SeqCst), first_llm_calls);

        // Verify SSE received digest_ready.
        let mut saw_digest_ready = false;
        while let Ok(env) = sse_rx.try_recv() {
            if env.event_type == digest_core::domain::SseEventType::DigestReady {
                saw_digest_ready = true;
            }
        }
        assert!(saw_digest_ready);
    }

    #[tokio::test]
    async fn composer_uses_deterministic_fallback_when_ai_unset_and_respects_email_enabled() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/fallback.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Feed</title><item><guid>f1</guid><title>Gemini Fallback Item</title><link>https://example.com/fb</link><description>Fallback test</description></item></channel></rss>"#,
            ))
            .mount(&server)
            .await;

        let webhook_hits = Arc::new(AtomicUsize::new(0));
        let webhook_hits_clone = Arc::clone(&webhook_hits);
        Mock::given(method("POST"))
            .and(path("/im-hook"))
            .respond_with(move |_req: &Request| {
                webhook_hits_clone.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
            })
            .mount(&server)
            .await;

        let mock = MockRepos::new();
        let llm_hits = Arc::new(AtomicUsize::new(0));
        let deps = build_deps(&mock, &server, Arc::clone(&llm_hits));

        let user_id = Uuid::new_v4();
        // email_enabled = false -> SMTP channel must be skipped; Webhook still delivered.
        deps.repos
            .settings
            .upsert_schedule(
                user_id,
                UpdateScheduleCmd {
                    timezone: "UTC".to_owned(),
                    daily_time: "08:30".to_owned(),
                    email_enabled: false,
                    send_when_empty: false,
                    alert_scan_enabled: true,
                    alert_interval_minutes: 60,
                },
            )
            .await
            .unwrap();

        let src_enc = encode_source_config(
            &deps.cipher,
            &format!("{}/fallback.xml", server.uri()),
            None,
        )
        .unwrap();
        deps.repos
            .sources
            .create_or_subscribe(
                user_id,
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "fb-rss".to_owned(),
                    custom_name: "Feed".to_owned(),
                    default_name: "Feed".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: format!("{}/fallback.xml", server.uri()),
                    config_json_enc: Some(src_enc),
                    enabled: true,
                },
            )
            .await
            .unwrap();

        let smtp_enc = deps
            .cipher
            .encrypt(
                &serde_json::json!({
                    "host": "smtp.example.com",
                    "port": 587,
                    "from": "digest@example.com",
                    "to": ["user@example.com"]
                })
                .to_string(),
            )
            .unwrap();
        deps.repos
            .settings
            .create_channel(
                user_id,
                CreateChannelRecord {
                    channel_type: NotificationChannelType::Smtp,
                    name: "Email".to_owned(),
                    enabled: true,
                    config_enc: smtp_enc,
                    config_hint: "user@example.com".to_owned(),
                    trigger_on_daily: true,
                    trigger_on_alert: true,
                },
            )
            .await
            .unwrap();

        let hook_enc = deps
            .cipher
            .encrypt(&serde_json::json!({ "url": format!("{}/im-hook", server.uri()) }).to_string())
            .unwrap();
        deps.repos
            .settings
            .create_channel(
                user_id,
                CreateChannelRecord {
                    channel_type: NotificationChannelType::Webhook,
                    name: "IM".to_owned(),
                    enabled: true,
                    config_enc: hook_enc,
                    config_hint: "https://example.com".to_owned(),
                    trigger_on_daily: true,
                    trigger_on_alert: true,
                },
            )
            .await
            .unwrap();

        let date = Utc::now().date_naive();
        let outcome = compose_user_digest(
            &deps,
            user_id,
            date,
            RunType::ManualDaily,
            false,
            true,
            None,
        )
        .await
        .unwrap();

        let digest = outcome.digest.unwrap();
        assert!(digest.degraded_fallback);
        assert_eq!(outcome.run.prompt_tokens, 0);
        assert_eq!(outcome.run.completion_tokens, 0);
        // Only Webhook dispatched, SMTP skipped due to email_enabled = false.
        assert_eq!(outcome.deliveries.len(), 1);
        assert_eq!(
            outcome.deliveries[0].channel_type,
            NotificationChannelType::Webhook
        );
        assert_eq!(outcome.deliveries[0].status, DeliveryStatus::Sent);
        assert_eq!(webhook_hits.load(Ordering::SeqCst), 1);
    }
}
