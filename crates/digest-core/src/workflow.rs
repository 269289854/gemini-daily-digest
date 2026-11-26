//! `DigestExecutionWorkflow`: Template Method orchestration of the full digest lifecycle
//! (`collect -> process -> summarize -> render -> notify -> status`).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use uuid::Uuid;

use crate::cache::CacheCoordinator;
use crate::collectors::CollectorRegistry;
use crate::config::{AppConfig, ResourceLimitsTuning};
use crate::domain::{
    AlertPriority, DailyDigestSynthesisInput, DeliveryOutcome, DeliveryStatus, DigestSection,
    DigestWorkflowInput, DigestWorkflowOutput, FetchOutcome, NotifierConfig, ProcessedItem,
    RenderInput, RunStatus, SourceDigestInput, SourceFetchRecord, SourceRunInput,
    SourceSummaryResult,
};
use crate::error::CoreError;
use crate::llm::{compute_input_fingerprint, IdempotentLlmRunner, OpenAiCompatProvider};
use crate::notifiers::NotifierRegistry;
use crate::pipeline::{
    deduplicate_processed_items, priority_rank, sort_processed_items, ItemProcessingPipeline,
};
use crate::renderer::RendererRegistry;
use crate::traits::{
    DigestRenderer, RawCollectedItem, RenderedNotification, SourceDescriptor,
    StructuredItemProcessor, StructuredLlmProvider, SummarizedDigest,
};

/// Raw collected items associated with a single source from Step 1.
#[derive(Debug, Clone)]
pub struct SourceRawBatch {
    pub source: SourceDescriptor,
    pub items: Vec<RawCollectedItem>,
}

/// Output of Step 1 (`collect_sources`).
#[derive(Debug, Clone)]
pub struct CollectedStage {
    pub source_fetches: Vec<SourceFetchRecord>,
    pub per_source_raw: Vec<SourceRawBatch>,
    pub items_fetched: usize,
}

/// Processed items grouped per source alongside globally deduplicated items from Step 2.
#[derive(Debug, Clone)]
pub struct ProcessedStage {
    pub per_source_processed: Vec<(SourceDescriptor, Vec<ProcessedItem>)>,
    pub processed_items: Vec<ProcessedItem>,
    pub highlighted_items: Vec<ProcessedItem>,
    pub items_new: usize,
    pub highlighted_count: usize,
}

/// Output of Step 3 (`summarize_digest`).
#[derive(Debug, Clone)]
pub struct SummarizedStage {
    pub input_fingerprint: String,
    pub source_summaries: Vec<SourceSummaryResult>,
    pub summarized_digest: SummarizedDigest,
    pub sections: Vec<DigestSection>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub cache_hit: bool,
}

/// Template Method contract defining the invariant 6-step digest execution lifecycle.
#[async_trait]
pub trait DigestWorkflowTemplate: Send + Sync {
    /// Template Method coordinating collect -> process -> summarize -> render -> notify -> status.
    async fn run_workflow(
        &self,
        input: &DigestWorkflowInput,
    ) -> Result<DigestWorkflowOutput, CoreError> {
        let collected = self.collect_sources(input).await?;
        let processed = self.process_collected(input, &collected).await?;
        let summarized = self.summarize_digest(input, &processed).await?;
        let rendered = self.render_digest(input, &processed, &summarized).await?;
        let deliveries = self
            .dispatch_notifications(input, &processed, rendered.as_ref())
            .await?;
        let status = self.compute_run_status(&collected.source_fetches, &deliveries);

        Ok(DigestWorkflowOutput {
            status,
            digest_date: input.digest_date,
            input_fingerprint: summarized.input_fingerprint,
            source_fetches: collected.source_fetches,
            processed_items: processed.processed_items,
            source_summaries: summarized.source_summaries,
            summarized_digest: summarized.summarized_digest,
            sections: summarized.sections,
            rendered,
            deliveries,
            items_fetched: collected.items_fetched,
            items_new: processed.items_new,
            highlighted_count: processed.highlighted_count,
            prompt_tokens: summarized.prompt_tokens,
            completion_tokens: summarized.completion_tokens,
            cache_hit: summarized.cache_hit,
        })
    }

    /// Step 1: Concurrently collect candidate items across configured sources with per-source error isolation.
    async fn collect_sources(
        &self,
        input: &DigestWorkflowInput,
    ) -> Result<CollectedStage, CoreError>;

    /// Step 2: Normalize, deduplicate, match watchlist rules, and score collected items.
    async fn process_collected(
        &self,
        input: &DigestWorkflowInput,
        collected: &CollectedStage,
    ) -> Result<ProcessedStage, CoreError>;

    /// Step 3: Summarize per-source and daily digest idempotently with deterministic fallback.
    async fn summarize_digest(
        &self,
        input: &DigestWorkflowInput,
        processed: &ProcessedStage,
    ) -> Result<SummarizedStage, CoreError>;

    /// Step 4: Render structured multi-format notification (`HTML`, `Markdown`, `PlainText`).
    async fn render_digest(
        &self,
        input: &DigestWorkflowInput,
        processed: &ProcessedStage,
        summarized: &SummarizedStage,
    ) -> Result<Option<RenderedNotification>, CoreError>;

    /// Step 5: Dispatch notifications to configured channels without aborting healthy channels on failure.
    async fn dispatch_notifications(
        &self,
        input: &DigestWorkflowInput,
        processed: &ProcessedStage,
        rendered: Option<&RenderedNotification>,
    ) -> Result<Vec<DeliveryOutcome>, CoreError>;

    /// Step 6: Compute overall `RunStatus` from source fetches and notification deliveries.
    fn compute_run_status(
        &self,
        source_fetches: &[SourceFetchRecord],
        deliveries: &[DeliveryOutcome],
    ) -> RunStatus {
        let all_sources_failed = !source_fetches.is_empty()
            && source_fetches
                .iter()
                .all(|record| record.outcome == FetchOutcome::Failed);
        if all_sources_failed {
            return RunStatus::Failed;
        }

        let any_source_failed = source_fetches
            .iter()
            .any(|record| record.outcome == FetchOutcome::Failed);
        let any_delivery_failed = deliveries
            .iter()
            .any(|outcome| outcome.status == DeliveryStatus::Failed);

        if any_source_failed || any_delivery_failed {
            RunStatus::Partial
        } else {
            RunStatus::Succeeded
        }
    }
}

/// Concrete Template Method workflow orchestrating collectors, processing pipeline,
/// idempotent LLM runner, multi-format renderers, and outbound notifiers.
pub struct DigestExecutionWorkflow {
    collectors: Arc<CollectorRegistry>,
    pipeline: Arc<dyn StructuredItemProcessor>,
    llm_runner: Arc<IdempotentLlmRunner>,
    renderer: Arc<dyn DigestRenderer>,
    notifiers: Arc<NotifierRegistry>,
    cache: Arc<dyn CacheCoordinator>,
    limits: ResourceLimitsTuning,
}

impl DigestExecutionWorkflow {
    /// Creates a `DigestExecutionWorkflow` with explicit registry/service dependencies.
    #[must_use]
    pub fn new(
        collectors: Arc<CollectorRegistry>,
        pipeline: Arc<dyn StructuredItemProcessor>,
        llm_runner: Arc<IdempotentLlmRunner>,
        renderer: Arc<dyn DigestRenderer>,
        notifiers: Arc<NotifierRegistry>,
        cache: Arc<dyn CacheCoordinator>,
        limits: ResourceLimitsTuning,
    ) -> Self {
        Self {
            collectors,
            pipeline,
            llm_runner,
            renderer,
            notifiers,
            cache,
            limits,
        }
    }

    /// Creates a `DigestExecutionWorkflow` with standard `ItemProcessingPipeline`,
    /// `IdempotentLlmRunner`, and `RendererRegistry` wired around the supplied registries.
    #[must_use]
    pub fn with_defaults(
        collectors: CollectorRegistry,
        llm_provider: Arc<dyn StructuredLlmProvider>,
        notifiers: NotifierRegistry,
        cache: Arc<dyn CacheCoordinator>,
        limits: ResourceLimitsTuning,
    ) -> Self {
        let llm_runner = Arc::new(IdempotentLlmRunner::with_ttl(
            Arc::clone(&cache),
            llm_provider,
            limits.digest_fingerprint_ttl,
        ));
        Self::new(
            Arc::new(collectors),
            Arc::new(ItemProcessingPipeline::new()),
            llm_runner,
            Arc::new(RendererRegistry::new()),
            Arc::new(notifiers),
            cache,
            limits,
        )
    }

    /// Constructs a `DigestExecutionWorkflow` directly from `AppConfig` and a `CacheCoordinator`.
    pub fn from_config(
        config: &AppConfig,
        cache: Arc<dyn CacheCoordinator>,
    ) -> Result<Self, CoreError> {
        let collectors = CollectorRegistry::from_config(config)?;
        let llm_provider: Arc<dyn StructuredLlmProvider> =
            Arc::new(OpenAiCompatProvider::from_config(config)?);
        let notifiers = NotifierRegistry::from_config(config)?;
        Ok(Self::with_defaults(
            collectors,
            llm_provider,
            notifiers,
            cache,
            config.resources.clone(),
        ))
    }

    /// Returns a reference to the underlying `CacheCoordinator`.
    #[must_use]
    pub fn cache(&self) -> &Arc<dyn CacheCoordinator> {
        &self.cache
    }

    /// Executes the full digest workflow for `input`.
    pub async fn execute(
        &self,
        input: &DigestWorkflowInput,
    ) -> Result<DigestWorkflowOutput, CoreError> {
        self.run_workflow(input).await
    }
}

#[async_trait]
impl DigestWorkflowTemplate for DigestExecutionWorkflow {
    async fn collect_sources(
        &self,
        input: &DigestWorkflowInput,
    ) -> Result<CollectedStage, CoreError> {
        let concurrency = self.limits.global_fetch_concurrency.get().max(1);
        let collectors = Arc::clone(&self.collectors);
        let sources: Vec<(usize, SourceRunInput)> =
            input.sources.iter().cloned().enumerate().collect();

        let mut indexed_results: Vec<(usize, SourceFetchRecord, Option<SourceRawBatch>)> =
            stream::iter(sources)
                .map(|(idx, source_input)| {
                    let collectors = Arc::clone(&collectors);
                    async move {
                        let source = &source_input.source;
                        let collector = match collectors.get(source.source_type) {
                            Some(found) => found,
                            None => {
                                return (
                                    idx,
                                    SourceFetchRecord {
                                        source_id: source.id,
                                        outcome: FetchOutcome::Failed,
                                        etag: source_input.conditional.etag.clone(),
                                        last_modified: source_input
                                            .conditional
                                            .last_modified
                                            .clone(),
                                        items_fetched: 0,
                                        error_message: Some(format!(
                                            "no collector registered for source type {}",
                                            source.source_type
                                        )),
                                    },
                                    None,
                                );
                            }
                        };

                        match collector
                            .fetch_with_credentials(
                                source,
                                &source_input.conditional,
                                source_input.credentials_json.as_ref(),
                            )
                            .await
                        {
                            Ok(fetch_result) => {
                                let count = fetch_result.items.len();
                                let record = SourceFetchRecord {
                                    source_id: source.id,
                                    outcome: fetch_result.outcome,
                                    etag: fetch_result.etag,
                                    last_modified: fetch_result.last_modified,
                                    items_fetched: count,
                                    error_message: None,
                                };
                                let batch = SourceRawBatch {
                                    source: source.clone(),
                                    items: fetch_result.items,
                                };
                                (idx, record, Some(batch))
                            }
                            Err(err) => {
                                tracing::warn!(
                                    source_id = %source.id,
                                    source_type = %source.source_type,
                                    error = %err,
                                    "source collection failed; isolating error"
                                );
                                let record = SourceFetchRecord {
                                    source_id: source.id,
                                    outcome: FetchOutcome::Failed,
                                    etag: source_input.conditional.etag.clone(),
                                    last_modified: source_input.conditional.last_modified.clone(),
                                    items_fetched: 0,
                                    error_message: Some(err.to_string()),
                                };
                                (idx, record, None)
                            }
                        }
                    }
                })
                .buffer_unordered(concurrency)
                .collect()
                .await;

        indexed_results.sort_by_key(|(idx, _, _)| *idx);

        let mut source_fetches = Vec::with_capacity(indexed_results.len());
        let mut per_source_raw = Vec::new();
        let mut items_fetched = 0usize;

        for (_, record, maybe_batch) in indexed_results {
            items_fetched = items_fetched.saturating_add(record.items_fetched);
            source_fetches.push(record);
            if let Some(batch) = maybe_batch {
                per_source_raw.push(batch);
            }
        }

        Ok(CollectedStage {
            source_fetches,
            per_source_raw,
            items_fetched,
        })
    }

    async fn process_collected(
        &self,
        input: &DigestWorkflowInput,
        collected: &CollectedStage,
    ) -> Result<ProcessedStage, CoreError> {
        let mut per_source_processed = Vec::with_capacity(collected.per_source_raw.len());
        let mut combined = Vec::new();

        for batch in &collected.per_source_raw {
            if batch.items.is_empty() {
                per_source_processed.push((batch.source.clone(), Vec::new()));
                continue;
            }
            let processed = self
                .pipeline
                .process_items(
                    Some(&batch.source),
                    batch.items.clone(),
                    &input.watchlist_rules,
                )
                .await?;
            combined.extend(processed.iter().cloned());
            per_source_processed.push((batch.source.clone(), processed));
        }

        let mut processed_items = deduplicate_processed_items(combined);
        sort_processed_items(&mut processed_items);

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

        let items_new = processed_items.len();
        let highlighted_count = highlighted_items.len();

        Ok(ProcessedStage {
            per_source_processed,
            processed_items,
            highlighted_items,
            items_new,
            highlighted_count,
        })
    }

    async fn summarize_digest(
        &self,
        input: &DigestWorkflowInput,
        processed: &ProcessedStage,
    ) -> Result<SummarizedStage, CoreError> {
        let input_fingerprint = compute_input_fingerprint(
            input.digest_date,
            &input.ai_config,
            &processed.processed_items,
        );

        if processed.processed_items.is_empty() {
            return Ok(SummarizedStage {
                input_fingerprint,
                source_summaries: Vec::new(),
                summarized_digest: SummarizedDigest {
                    title: format!("Gemini Daily Digest — {}", input.digest_date),
                    overview_markdown: format!("No new items collected for {}.", input.digest_date),
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    model_used: input.ai_config.model.clone(),
                    degraded_fallback: false,
                },
                sections: Vec::new(),
                prompt_tokens: 0,
                completion_tokens: 0,
                cache_hit: false,
            });
        }

        let concurrency = self.limits.global_fetch_concurrency.get().max(1);
        let non_empty_sources: Vec<(usize, SourceDigestInput)> = processed
            .per_source_processed
            .iter()
            .filter(|(_, items)| !items.is_empty())
            .enumerate()
            .map(|(idx, (source, items))| {
                (
                    idx,
                    SourceDigestInput {
                        source: source.clone(),
                        digest_date: input.digest_date,
                        items: items.clone(),
                    },
                )
            })
            .collect();

        let llm_runner = Arc::clone(&self.llm_runner);
        let ai_config = input.ai_config.clone();
        let force_refresh = input.force_refresh;

        let mut indexed_summaries: Vec<(usize, Result<SourceSummaryResult, CoreError>)> =
            stream::iter(non_empty_sources)
                .map(|(idx, source_input)| {
                    let llm_runner = Arc::clone(&llm_runner);
                    let ai_config = ai_config.clone();
                    async move {
                        let res = llm_runner
                            .summarize_source_idempotent(&ai_config, &source_input, force_refresh)
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
            digest_date: input.digest_date,
            source_summaries: source_summaries.clone(),
            highlighted_items: processed.highlighted_items.clone(),
            all_items: processed.processed_items.clone(),
        };

        let daily_outcome = self
            .llm_runner
            .summarize_daily_with_options(&input.ai_config, &daily_input, input.force_refresh)
            .await?;

        let mut items_by_source: HashMap<Uuid, Vec<ProcessedItem>> = HashMap::new();
        for (source, items) in &processed.per_source_processed {
            if !items.is_empty() {
                items_by_source.insert(source.id, items.clone());
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

        Ok(SummarizedStage {
            input_fingerprint: daily_outcome.input_fingerprint,
            source_summaries,
            summarized_digest: daily_outcome.digest,
            sections,
            prompt_tokens,
            completion_tokens,
            cache_hit: daily_outcome.cache_hit,
        })
    }

    async fn render_digest(
        &self,
        input: &DigestWorkflowInput,
        processed: &ProcessedStage,
        summarized: &SummarizedStage,
    ) -> Result<Option<RenderedNotification>, CoreError> {
        let highest_priority = processed
            .processed_items
            .iter()
            .map(|item| item.match_result.priority)
            .max_by_key(|priority| priority_rank(*priority))
            .unwrap_or(AlertPriority::Normal);

        let render_input = RenderInput {
            digest_date: input.digest_date,
            digest: summarized.summarized_digest.clone(),
            sections: summarized.sections.clone(),
            highlighted_items: processed.highlighted_items.clone(),
            all_items: processed.processed_items.clone(),
            priority: highest_priority,
            locale: input.ai_config.output_language.clone(),
        };

        let rendered = self.renderer.render_structured(&render_input)?;
        Ok(Some(rendered))
    }

    async fn dispatch_notifications(
        &self,
        input: &DigestWorkflowInput,
        processed: &ProcessedStage,
        rendered: Option<&RenderedNotification>,
    ) -> Result<Vec<DeliveryOutcome>, CoreError> {
        if input.notifiers.is_empty()
            || (processed.processed_items.is_empty() && !input.send_when_empty)
        {
            return Ok(Vec::new());
        }

        let Some(payload) = rendered else {
            return Ok(Vec::new());
        };

        let concurrency = self.limits.global_fetch_concurrency.get().max(1);
        let notifiers = Arc::clone(&self.notifiers);
        let channels: Vec<(usize, NotifierConfig)> =
            input.notifiers.iter().cloned().enumerate().collect();
        let payload = payload.clone();

        let mut indexed_deliveries: Vec<(usize, DeliveryOutcome)> = stream::iter(channels)
            .map(|(idx, channel)| {
                let notifiers = Arc::clone(&notifiers);
                let payload = payload.clone();
                async move {
                    let outcome = match notifiers.send(&channel, &payload).await {
                        Ok(()) => DeliveryOutcome {
                            channel_id: channel.channel_id,
                            channel_type: channel.channel_type,
                            status: DeliveryStatus::Sent,
                            error_message: None,
                            sent_at: Utc::now(),
                        },
                        Err(err) => {
                            tracing::warn!(
                                channel_type = %channel.channel_type,
                                channel_name = %channel.name,
                                error = %err,
                                "notification delivery failed; isolating error"
                            );
                            DeliveryOutcome {
                                channel_id: channel.channel_id,
                                channel_type: channel.channel_type,
                                status: DeliveryStatus::Failed,
                                error_message: Some(err.to_string()),
                                sent_at: Utc::now(),
                            }
                        }
                    };
                    (idx, outcome)
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

        indexed_deliveries.sort_by_key(|(idx, _)| *idx);
        Ok(indexed_deliveries
            .into_iter()
            .map(|(_, outcome)| outcome)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::time::Duration;

    use chrono::NaiveDate;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cache::InMemoryCacheCoordinator;
    use crate::collectors::CollectorContext;
    use crate::config::{CollectorTuning, LlmTuning, NotifierTuning};
    use crate::constants::FALLBACK_MODEL_LABEL;
    use crate::domain::{
        NotificationChannelType, NotifierConfig, RunType, SourceRunInput, SourceScope, SourceType,
        UserAiConfig, WatchlistRuleInput,
    };
    use crate::http::shared_http_client;
    use crate::ssrf::SsrfGuard;
    use crate::traits::ConditionalFetchHeaders;

    const SAMPLE_RSS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Gemini Official Feed</title>
    <item>
      <guid>item-rss-1</guid>
      <title>Gemini 2.5 Pro GA Launch</title>
      <link>https://blog.google/technology/ai/gemini-2-5-pro/?utm_source=rss</link>
      <description>Gemini 2.5 Pro introduces major reasoning upgrades.</description>
      <pubDate>Thu, 26 Nov 2026 08:00:00 +0000</pubDate>
    </item>
  </channel>
</rss>"#;

    fn local_ssrf_guard() -> SsrfGuard {
        SsrfGuard::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()])
    }

    fn build_test_workflow(
        server: &MockServer,
        cache: Arc<dyn CacheCoordinator>,
        llm_timeout: Duration,
    ) -> DigestExecutionWorkflow {
        let collector_tuning = CollectorTuning {
            initial_backoff: Duration::from_millis(5),
            max_retry_attempts: NonZeroUsize::new(1).unwrap(),
            ..CollectorTuning::default()
        };
        let limits = ResourceLimitsTuning::default();
        let client = shared_http_client(None, &collector_tuning).unwrap();
        let ssrf = local_ssrf_guard();

        let collector_ctx = CollectorContext::new(
            client.clone(),
            ssrf.clone(),
            collector_tuning,
            limits.clone(),
        )
        .with_github_api_base_url(server.uri());
        let collectors = CollectorRegistry::new(collector_ctx);

        let llm_tuning = LlmTuning {
            request_timeout: llm_timeout,
            ..LlmTuning::default()
        };
        let llm_provider: Arc<dyn StructuredLlmProvider> =
            Arc::new(OpenAiCompatProvider::with_tuning(
                client.clone(),
                ssrf.clone(),
                llm_tuning,
                limits.max_feed_bytes.get(),
            ));

        let notifiers = NotifierRegistry::with_defaults(client, ssrf, NotifierTuning::default());

        DigestExecutionWorkflow::with_defaults(collectors, llm_provider, notifiers, cache, limits)
    }

    fn rss_source(url: String) -> SourceRunInput {
        SourceRunInput {
            source: SourceDescriptor {
                id: Uuid::new_v4(),
                source_type: SourceType::Rss,
                scope: SourceScope::SharedPublic,
                canonical_key: "rss:gemini-official".to_owned(),
                name: "Gemini Official Blog".to_owned(),
                handle_or_url: url,
                category: "Official".to_owned(),
            },
            conditional: ConditionalFetchHeaders::default(),
            credentials_json: None,
        }
    }

    fn github_source(repo: &str) -> SourceRunInput {
        SourceRunInput {
            source: SourceDescriptor {
                id: Uuid::new_v4(),
                source_type: SourceType::GithubRelease,
                scope: SourceScope::SharedPublic,
                canonical_key: format!("github_release:{repo}"),
                name: "Python GenAI SDK".to_owned(),
                handle_or_url: repo.to_owned(),
                category: "Releases".to_owned(),
            },
            conditional: ConditionalFetchHeaders::default(),
            credentials_json: None,
        }
    }

    fn sample_ai_config(base_url: &str) -> UserAiConfig {
        UserAiConfig {
            provider_label: "MockAI".to_owned(),
            base_url: base_url.to_owned(),
            api_key: Some("test-key-placeholder".to_owned()),
            model: "deepseek-chat".to_owned(),
            system_prompt: "Summarize Gemini ecosystem updates concisely.".to_owned(),
            temperature: 0.2,
            max_tokens: 1024,
            output_language: "en-US".to_owned(),
        }
    }

    fn sample_watchlist() -> Vec<WatchlistRuleInput> {
        vec![WatchlistRuleInput {
            id: None,
            name: "Gemini 2.5".to_owned(),
            keywords: vec!["Gemini 2.5".to_owned()],
            exclude_keywords: Vec::new(),
            priority: AlertPriority::Urgent,
            notify_immediately: true,
            enabled: true,
        }]
    }

    fn sample_webhook_notifier(url: String) -> NotifierConfig {
        NotifierConfig {
            channel_id: Some(Uuid::new_v4()),
            channel_type: NotificationChannelType::Webhook,
            name: "Primary Webhook".to_owned(),
            config_json: serde_json::json!({
                "url": url,
                "secret": "test-key-placeholder"
            }),
        }
    }

    async fn mount_healthy_sources_and_llm(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/rss.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(SAMPLE_RSS_XML))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/googleapis/python-genai/releases"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": 401,
                    "tag_name": "v1.0.0",
                    "name": "python-genai v1.0.0",
                    "body": "Official Python SDK for Gemini.",
                    "html_url": "https://github.com/googleapis/python-genai/releases/tag/v1.0.0",
                    "published_at": "2026-11-26T07:00:00Z",
                    "draft": false,
                    "author": { "login": "google-bot" }
                }
            ])))
            .mount(server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "deepseek-chat",
                "choices": [{
                    "message": {
                        "content": "## Executive Summary\n- Gemini 2.5 Pro GA and python-genai v1.0.0 shipped."
                    }
                }],
                "usage": {
                    "prompt_tokens": 100,
                    "completion_tokens": 40
                }
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn end_to_end_happy_path_and_second_run_hits_fingerprint_cache_with_zero_tokens() {
        let server = MockServer::start().await;
        mount_healthy_sources_and_llm(&server).await;

        Mock::given(method("POST"))
            .and(path("/webhook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(2)
            .mount(&server)
            .await;

        let cache = Arc::new(InMemoryCacheCoordinator::new());
        let workflow = build_test_workflow(&server, cache, Duration::from_secs(5));

        let input = DigestWorkflowInput {
            user_id: Some(Uuid::new_v4()),
            digest_date: NaiveDate::from_ymd_opt(2026, 11, 26).unwrap(),
            run_type: RunType::ManualDaily,
            sources: vec![
                rss_source(format!("{}/rss.xml", server.uri())),
                github_source("googleapis/python-genai"),
            ],
            watchlist_rules: sample_watchlist(),
            ai_config: sample_ai_config(&server.uri()),
            notifiers: vec![sample_webhook_notifier(format!("{}/webhook", server.uri()))],
            send_when_empty: false,
            force_refresh: false,
        };

        let first = workflow.execute(&input).await.unwrap();
        assert_eq!(first.status, RunStatus::Succeeded);
        assert_eq!(first.items_fetched, 2);
        assert_eq!(first.items_new, 2);
        assert_eq!(first.highlighted_count, 1);
        assert_eq!(first.source_summaries.len(), 2);
        assert_eq!(first.sections.len(), 2);
        assert!(!first.cache_hit);
        assert!(!first.summarized_digest.degraded_fallback);
        // 2 per-source calls + 1 daily synthesis call = 3 * (100, 40)
        assert_eq!(first.prompt_tokens, 300);
        assert_eq!(first.completion_tokens, 120);
        assert_eq!(first.deliveries.len(), 1);
        assert_eq!(first.deliveries[0].status, DeliveryStatus::Sent);

        let rendered = first.rendered.as_ref().unwrap();
        assert!(rendered.subject.starts_with("[URGENT]"));
        assert!(rendered.html_body.contains("Gemini 2.5 Pro GA Launch"));
        assert!(rendered.markdown_body.contains("python-genai v1.0.0"));
        assert!(rendered.text_body.contains("EXECUTIVE OVERVIEW"));

        // Second identical run must hit fingerprint cache with 0 LLM tokens.
        let second = workflow.execute(&input).await.unwrap();
        assert_eq!(second.status, RunStatus::Succeeded);
        assert_eq!(second.input_fingerprint, first.input_fingerprint);
        assert!(second.cache_hit);
        assert_eq!(second.prompt_tokens, 0);
        assert_eq!(second.completion_tokens, 0);
        assert_eq!(
            second.summarized_digest.overview_markdown,
            first.summarized_digest.overview_markdown
        );
        assert!(second
            .source_summaries
            .iter()
            .all(|s| s.prompt_tokens == 0 && s.completion_tokens == 0));
    }

    #[tokio::test]
    async fn llm_500_triggers_deterministic_fallback_cleanly() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rss.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(SAMPLE_RSS_XML))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("upstream error"))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/webhook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let cache = Arc::new(InMemoryCacheCoordinator::new());
        let workflow = build_test_workflow(&server, cache, Duration::from_secs(2));

        let input = DigestWorkflowInput {
            user_id: None,
            digest_date: NaiveDate::from_ymd_opt(2026, 11, 26).unwrap(),
            run_type: RunType::ScheduledDaily,
            sources: vec![rss_source(format!("{}/rss.xml", server.uri()))],
            watchlist_rules: sample_watchlist(),
            ai_config: sample_ai_config(&server.uri()),
            notifiers: vec![sample_webhook_notifier(format!("{}/webhook", server.uri()))],
            send_when_empty: false,
            force_refresh: false,
        };

        let output = workflow.execute(&input).await.unwrap();
        assert_eq!(output.status, RunStatus::Succeeded);
        assert!(output.summarized_digest.degraded_fallback);
        assert_eq!(output.summarized_digest.model_used, FALLBACK_MODEL_LABEL);
        assert_eq!(output.prompt_tokens, 0);
        assert_eq!(output.completion_tokens, 0);
        assert!(output
            .source_summaries
            .iter()
            .all(|s| s.degraded_fallback && s.model_used == FALLBACK_MODEL_LABEL));
        assert!(output
            .rendered
            .as_ref()
            .unwrap()
            .html_body
            .contains("Fallback Mode"));
    }

    #[tokio::test]
    async fn partial_source_and_notifier_failures_isolate_healthy_paths() {
        let server = MockServer::start().await;
        mount_healthy_sources_and_llm(&server).await;

        // Failing second source (404)
        Mock::given(method("GET"))
            .and(path("/broken.xml"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        // Healthy webhook + failing webhook
        Mock::given(method("POST"))
            .and(path("/webhook-ok"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/webhook-fail"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let cache = Arc::new(InMemoryCacheCoordinator::new());
        let workflow = build_test_workflow(&server, cache, Duration::from_secs(5));

        let input = DigestWorkflowInput {
            user_id: Some(Uuid::new_v4()),
            digest_date: NaiveDate::from_ymd_opt(2026, 11, 26).unwrap(),
            run_type: RunType::ManualDaily,
            sources: vec![
                rss_source(format!("{}/rss.xml", server.uri())),
                rss_source(format!("{}/broken.xml", server.uri())),
            ],
            watchlist_rules: sample_watchlist(),
            ai_config: sample_ai_config(&server.uri()),
            notifiers: vec![
                sample_webhook_notifier(format!("{}/webhook-fail", server.uri())),
                sample_webhook_notifier(format!("{}/webhook-ok", server.uri())),
            ],
            send_when_empty: false,
            force_refresh: false,
        };

        let output = workflow.execute(&input).await.unwrap();
        assert_eq!(output.status, RunStatus::Partial);
        assert_eq!(output.source_fetches.len(), 2);
        assert_eq!(output.source_fetches[0].outcome, FetchOutcome::Updated);
        assert_eq!(output.source_fetches[1].outcome, FetchOutcome::Failed);
        assert!(output.source_fetches[1].error_message.is_some());
        assert_eq!(output.items_new, 1);

        assert_eq!(output.deliveries.len(), 2);
        assert_eq!(output.deliveries[0].status, DeliveryStatus::Failed);
        assert_eq!(output.deliveries[1].status, DeliveryStatus::Sent);
    }

    #[tokio::test]
    async fn all_sources_failed_returns_failed_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/broken.xml"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let cache = Arc::new(InMemoryCacheCoordinator::new());
        let workflow = build_test_workflow(&server, cache, Duration::from_secs(2));

        let input = DigestWorkflowInput {
            user_id: None,
            digest_date: NaiveDate::from_ymd_opt(2026, 11, 26).unwrap(),
            run_type: RunType::ScheduledDaily,
            sources: vec![rss_source(format!("{}/broken.xml", server.uri()))],
            watchlist_rules: Vec::new(),
            ai_config: sample_ai_config(&server.uri()),
            notifiers: vec![sample_webhook_notifier(format!("{}/webhook", server.uri()))],
            send_when_empty: false,
            force_refresh: false,
        };

        let output = workflow.execute(&input).await.unwrap();
        assert_eq!(output.status, RunStatus::Failed);
        assert_eq!(output.items_fetched, 0);
        assert!(output.deliveries.is_empty());
    }

    #[tokio::test]
    async fn empty_items_with_send_when_empty_false_skips_notifier_delivery() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/empty.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>Empty Feed</title></channel></rss>"#,
            ))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/webhook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let cache = Arc::new(InMemoryCacheCoordinator::new());
        let workflow = build_test_workflow(&server, cache, Duration::from_secs(2));

        let input = DigestWorkflowInput {
            user_id: None,
            digest_date: NaiveDate::from_ymd_opt(2026, 11, 26).unwrap(),
            run_type: RunType::ScheduledDaily,
            sources: vec![rss_source(format!("{}/empty.xml", server.uri()))],
            watchlist_rules: Vec::new(),
            ai_config: sample_ai_config(&server.uri()),
            notifiers: vec![sample_webhook_notifier(format!("{}/webhook", server.uri()))],
            send_when_empty: false,
            force_refresh: false,
        };

        let output = workflow.execute(&input).await.unwrap();
        assert_eq!(output.status, RunStatus::Succeeded);
        assert_eq!(output.items_fetched, 0);
        assert_eq!(output.items_new, 0);
        assert_eq!(output.prompt_tokens, 0);
        assert_eq!(output.completion_tokens, 0);
        assert!(output.deliveries.is_empty());
    }
}
