//! `ServerExecutionOrchestrator` implementing `ExecutionOrchestrator` for manual runs,
//! AI connectivity tests, single-source live previews, channel tests, and delivery resends.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use chrono_tz::Tz;
use digest_core::constants::{
    DEFAULT_AI_BASE_URL, DEFAULT_AI_MODEL, DEFAULT_AI_PROVIDER_LABEL, DEFAULT_LLM_MAX_TOKENS,
    DEFAULT_LLM_SYSTEM_PROMPT, DEFAULT_LLM_TEMPERATURE, DEFAULT_MAX_PAGE_SIZE, DEFAULT_TIMEZONE,
    SECRET_MASK_REDACTED_PLACEHOLDER,
};
use digest_core::domain::{
    AlertPriority, DeliveryStatus, FetchOutcome, NotifierConfig, ProcessedItem, RenderInput,
    RunStatus, RunType, UserAiConfig,
};
use digest_core::error::CoreError;
use digest_core::llm::OpenAiCompatProvider;
use digest_core::pipeline::priority_rank;
use digest_core::presets::find_preset_by_key;
use digest_core::renderer::RendererRegistry;
use digest_core::ssrf::SsrfGuard;
use digest_core::traits::{
    ConditionalFetchHeaders, DigestRenderer, RenderedNotification, SourceDescriptor,
    StructuredLlmProvider, SummarizedDigest,
};
use uuid::Uuid;

use crate::constants::SOURCE_TEST_SAMPLE_TITLE_LIMIT;
use crate::repo::{
    CanonicalSourceRecord, CreateRunCmd, DailyDigestDetail, DeliveryLogRecord, DigestRunView,
    PageRequest,
};
use crate::scheduler::composer::{
    compose_user_digest, resolve_user_ai_config_blocking, ComposeOutcome, ComposerDeps,
};
use crate::scheduler::poller::{
    decode_source_payload_blocking, fetch_with_platform_x_support,
    poll_canonical_source_once_with_platform_x, EffectivePlatformXConfig, SourcePollOutcome,
};
use crate::services::{
    AiModelsRequest, AiModelsResult, AiTestRequest, AiTestResult, ChannelTestResult,
    ExecutionOrchestrator, SourceTestResult, TriggerRunRequest,
};
use crate::sse_hub::run_progress_envelope;

const STAGE_QUEUED: &str = "queued";
const TEST_RENDER_MODEL_LABEL: &str = "test-preview";

/// Concrete `ExecutionOrchestrator` coordinating manual digest runs, live probes, and resends.
#[derive(Clone)]
pub struct ServerExecutionOrchestrator {
    deps: ComposerDeps,
    ssrf: SsrfGuard,
    llm_provider: Arc<dyn StructuredLlmProvider>,
}

impl ServerExecutionOrchestrator {
    /// Builds a `ServerExecutionOrchestrator` from shared scheduler/composer dependencies.
    #[must_use]
    pub fn new(
        deps: ComposerDeps,
        ssrf: SsrfGuard,
        llm_provider: Arc<dyn StructuredLlmProvider>,
    ) -> Self {
        Self {
            deps,
            ssrf,
            llm_provider,
        }
    }

    /// Directly runs `compose_user_digest` inline (useful for deterministic integration tests).
    pub async fn compose_user_digest_inline(
        &self,
        user_id: Uuid,
        digest_date: NaiveDate,
        run_type: RunType,
        force_refresh: bool,
        notify: bool,
    ) -> Result<ComposeOutcome, CoreError> {
        compose_user_digest(
            &self.deps,
            user_id,
            digest_date,
            run_type,
            force_refresh,
            notify,
            None,
        )
        .await
    }

    /// Directly polls a single canonical source once (useful for deterministic integration tests).
    pub async fn poll_source_once(
        &self,
        source: &CanonicalSourceRecord,
        force_refresh: bool,
    ) -> Result<SourcePollOutcome, CoreError> {
        poll_canonical_source_once_with_platform_x(
            &self.deps.repos,
            &self.deps.cache,
            &self.deps.cipher,
            &self.deps.collectors,
            &self.deps.pipeline,
            &self.deps.config.core.collector,
            &self.deps.config.core.resources,
            source,
            force_refresh,
            self.deps.config.platform_x_bridge_base_url.as_deref(),
        )
        .await
    }

    async fn find_digest_by_id(
        &self,
        user_id: Uuid,
        digest_id: Uuid,
    ) -> Result<DailyDigestDetail, CoreError> {
        let mut page_num = 1usize;
        loop {
            let page = PageRequest::new(Some(page_num), Some(DEFAULT_MAX_PAGE_SIZE))?;
            let list = self
                .deps
                .repos
                .digests
                .list_daily_digests(user_id, page)
                .await?;
            if let Some(summary) = list.items.iter().find(|item| item.id == digest_id) {
                return self
                    .deps
                    .repos
                    .digests
                    .get_daily_digest_by_date(user_id, summary.digest_date)
                    .await?
                    .ok_or_else(|| {
                        CoreError::NotFound(format!("daily digest {digest_id} not found"))
                    });
            }
            if u64::try_from(page_num).unwrap_or(u64::MAX) >= list.total_pages
                || list.items.is_empty()
            {
                break;
            }
            page_num = page_num.saturating_add(1);
        }

        Err(CoreError::NotFound(format!(
            "daily digest {digest_id} not found"
        )))
    }
}

#[async_trait]
impl ExecutionOrchestrator for ServerExecutionOrchestrator {
    async fn trigger_manual_run(
        &self,
        user_id: Uuid,
        req: TriggerRunRequest,
    ) -> Result<DigestRunView, CoreError> {
        let digest_date = match req.digest_date {
            Some(date) => date,
            None => {
                let schedule = self.deps.repos.settings.get_schedule(user_id).await?;
                let tz: Tz = schedule
                    .timezone
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| DEFAULT_TIMEZONE.parse().unwrap_or(Tz::UTC));
                Utc::now().with_timezone(&tz).date_naive()
            }
        };

        let run = self
            .deps
            .repos
            .runs
            .create_run(CreateRunCmd {
                user_id,
                run_type: RunType::ManualDaily,
                started_at: Utc::now(),
            })
            .await?;

        self.deps.sse.publish(
            user_id,
            run_progress_envelope(
                run.id,
                RunType::ManualDaily,
                RunStatus::Running,
                STAGE_QUEUED,
                0,
                0,
                None,
            ),
        );

        let deps = self.deps.clone();
        let force_refresh = req.force_refresh;
        let notify = req.notify;
        let run_id = run.id;

        tokio::spawn(async move {
            if let Err(err) = compose_user_digest(
                &deps,
                user_id,
                digest_date,
                RunType::ManualDaily,
                force_refresh,
                notify,
                Some(run_id),
            )
            .await
            {
                tracing::error!(
                    user_id = %user_id,
                    run_id = %run_id,
                    error = %err,
                    "manual digest run failed"
                );
            }
        });

        Ok(DigestRunView {
            id: run.id,
            run_type: run.run_type,
            status: RunStatus::Running,
            started_at: run.started_at,
            finished_at: None,
            items_fetched: 0,
            items_new: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            error_summary: None,
            deliveries: Vec::new(),
        })
    }

    async fn test_ai_config(
        &self,
        user_id: Uuid,
        req: AiTestRequest,
    ) -> Result<AiTestResult, CoreError> {
        let stored = self.deps.repos.settings.get_ai_config(user_id).await?;
        let base_cfg = resolve_user_ai_config_blocking(
            Arc::clone(&self.deps.cipher),
            stored,
            self.deps.config.core.llm.default_max_tokens,
        )
        .await;

        let api_key = match req
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(override_key) => Some(override_key.to_owned()),
            None => base_cfg.api_key,
        };

        let ai_cfg = UserAiConfig {
            provider_label: req
                .provider_label
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    if base_cfg.provider_label.is_empty() {
                        DEFAULT_AI_PROVIDER_LABEL.to_owned()
                    } else {
                        base_cfg.provider_label
                    }
                }),
            base_url: req
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    if base_cfg.base_url.is_empty() {
                        DEFAULT_AI_BASE_URL.to_owned()
                    } else {
                        base_cfg.base_url
                    }
                }),
            api_key,
            model: req
                .model
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    if base_cfg.model.is_empty() {
                        DEFAULT_AI_MODEL.to_owned()
                    } else {
                        base_cfg.model
                    }
                }),
            system_prompt: req
                .system_prompt
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    if base_cfg.system_prompt.is_empty() {
                        DEFAULT_LLM_SYSTEM_PROMPT.to_owned()
                    } else {
                        base_cfg.system_prompt
                    }
                }),
            temperature: req.temperature.unwrap_or(if base_cfg.temperature >= 0.0 {
                base_cfg.temperature
            } else {
                DEFAULT_LLM_TEMPERATURE
            }),
            max_tokens: req.max_tokens.unwrap_or(if base_cfg.max_tokens > 0 {
                base_cfg.max_tokens
            } else {
                DEFAULT_LLM_MAX_TOKENS
            }),
            output_language: base_cfg.output_language,
        };

        let endpoint_url = OpenAiCompatProvider::build_chat_completions_url(&ai_cfg.base_url)?;
        self.ssrf.validate_url(&endpoint_url).await?;

        let started = Instant::now();
        let probe_res = self.llm_provider.test_connection(&ai_cfg).await;
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

        match probe_res {
            Ok(()) => Ok(AiTestResult {
                ok: true,
                provider_label: ai_cfg.provider_label,
                model: ai_cfg.model,
                latency_ms,
                message: "AI endpoint connectivity verified".to_owned(),
            }),
            Err(err @ CoreError::SsrfBlocked(_)) => Err(err),
            Err(err) => {
                let sanitized =
                    sanitize_secret_from_msg(&err.to_string(), ai_cfg.api_key.as_deref());
                Ok(AiTestResult {
                    ok: false,
                    provider_label: ai_cfg.provider_label,
                    model: ai_cfg.model,
                    latency_ms,
                    message: sanitized,
                })
            }
        }
    }

    async fn list_ai_models(
        &self,
        user_id: Uuid,
        req: AiModelsRequest,
    ) -> Result<AiModelsResult, CoreError> {
        let stored = self.deps.repos.settings.get_ai_config(user_id).await?;
        let base_cfg = resolve_user_ai_config_blocking(
            Arc::clone(&self.deps.cipher),
            stored,
            self.deps.config.core.llm.default_max_tokens,
        )
        .await;

        let api_key = if req.clear_api_key {
            None
        } else if let Some(override_key) = req
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(override_key.to_owned())
        } else {
            base_cfg.api_key
        };

        let base_url = req
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if base_cfg.base_url.is_empty() {
                    DEFAULT_AI_BASE_URL.to_owned()
                } else {
                    base_cfg.base_url
                }
            });

        let ai_cfg = UserAiConfig {
            provider_label: if base_cfg.provider_label.is_empty() {
                DEFAULT_AI_PROVIDER_LABEL.to_owned()
            } else {
                base_cfg.provider_label
            },
            base_url,
            api_key,
            model: if base_cfg.model.is_empty() {
                DEFAULT_AI_MODEL.to_owned()
            } else {
                base_cfg.model
            },
            system_prompt: if base_cfg.system_prompt.is_empty() {
                DEFAULT_LLM_SYSTEM_PROMPT.to_owned()
            } else {
                base_cfg.system_prompt
            },
            temperature: if base_cfg.temperature >= 0.0 {
                base_cfg.temperature
            } else {
                DEFAULT_LLM_TEMPERATURE
            },
            max_tokens: if base_cfg.max_tokens > 0 {
                base_cfg.max_tokens
            } else {
                DEFAULT_LLM_MAX_TOKENS
            },
            output_language: base_cfg.output_language,
        };

        let endpoint_url = OpenAiCompatProvider::build_models_url(&ai_cfg.base_url)?;
        self.ssrf.validate_url(&endpoint_url).await?;

        let started = Instant::now();
        let list_res = self.llm_provider.list_models(&ai_cfg).await;
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

        match list_res {
            Ok(models) => Ok(AiModelsResult {
                models,
                endpoint_url,
                latency_ms,
            }),
            Err(err @ CoreError::SsrfBlocked(_)) => Err(err),
            Err(err) => {
                let sanitized =
                    sanitize_secret_from_msg(&err.to_string(), ai_cfg.api_key.as_deref());
                match err {
                    CoreError::UpstreamTimeout(_) => Err(CoreError::UpstreamTimeout(sanitized)),
                    CoreError::InvalidConfig(_) => Err(CoreError::InvalidConfig(sanitized)),
                    _ => Err(CoreError::Upstream(sanitized)),
                }
            }
        }
    }

    async fn test_source(
        &self,
        user_id: Uuid,
        source_id: Uuid,
    ) -> Result<SourceTestResult, CoreError> {
        let user_src = self
            .deps
            .repos
            .sources
            .get_user_source(user_id, source_id)
            .await?
            .ok_or_else(|| CoreError::NotFound(format!("source {source_id} not found")))?;

        let payload = decode_source_payload_blocking(
            Arc::clone(&self.deps.cipher),
            user_src.canonical_key.clone(),
            user_src.config_json_enc.clone(),
        )
        .await?;

        let trimmed_target = payload.handle_or_url.trim();
        if trimmed_target.starts_with("http://") || trimmed_target.starts_with("https://") {
            self.ssrf.validate_url(trimmed_target).await?;
        }

        let category = if user_src.category.trim().is_empty() {
            find_preset_by_key(&user_src.canonical_key)
                .map(|p| p.category)
                .unwrap_or_else(|| "General".to_owned())
        } else {
            user_src.category.clone()
        };

        let descriptor = SourceDescriptor {
            id: user_src.canonical_source_id,
            source_type: user_src.source_type,
            scope: user_src.scope,
            canonical_key: user_src.canonical_key.clone(),
            name: user_src.custom_name.clone(),
            handle_or_url: payload.handle_or_url,
            category,
        };

        match fetch_with_platform_x_support(
            &self.deps.repos,
            &self.deps.cipher,
            &self.deps.collectors,
            &descriptor,
            &ConditionalFetchHeaders::default(),
            payload.credentials.as_ref(),
            None,
            self.deps.config.platform_x_bridge_base_url.as_deref(),
        )
        .await
        {
            Ok(res) => {
                let sample_titles = res
                    .items
                    .iter()
                    .take(SOURCE_TEST_SAMPLE_TITLE_LIMIT)
                    .map(|item| item.title.clone())
                    .collect();
                Ok(SourceTestResult {
                    source_id: user_src.canonical_source_id,
                    outcome: res.outcome,
                    items_fetched: res.items.len(),
                    sample_titles,
                    error_message: None,
                })
            }
            Err(err @ CoreError::SsrfBlocked(_)) => Err(err),
            Err(err) => Ok(SourceTestResult {
                source_id: user_src.canonical_source_id,
                outcome: FetchOutcome::Failed,
                items_fetched: 0,
                sample_titles: Vec::new(),
                error_message: Some(err.to_string()),
            }),
        }
    }

    async fn test_channel(
        &self,
        user_id: Uuid,
        channel_id: Uuid,
    ) -> Result<ChannelTestResult, CoreError> {
        let channel = self
            .deps
            .repos
            .settings
            .get_channel(user_id, channel_id)
            .await?
            .ok_or_else(|| {
                CoreError::NotFound(format!("notification channel {channel_id} not found"))
            })?;

        let cipher = Arc::clone(&self.deps.cipher);
        let config_enc = channel.config_enc.clone();
        let ch_name = channel.name.clone();
        let ch_type = channel.channel_type;

        let config_json = tokio::task::spawn_blocking(move || {
            let plain = cipher.decrypt(&config_enc)?;
            serde_json::from_str::<serde_json::Value>(&plain).map_err(|err| {
                CoreError::Crypto(format!("invalid decrypted channel config JSON: {err}"))
            })
        })
        .await
        .map_err(|err| CoreError::Internal(format!("channel decrypt task failed: {err}")))??;

        let rendered =
            render_channel_test_notification_blocking(Arc::clone(&self.deps.renderer), &ch_name)
                .await?;

        let notifier_cfg = NotifierConfig {
            channel_id: Some(channel.id),
            channel_type: ch_type,
            name: ch_name,
            config_json,
        };

        let sent_at = Utc::now();
        match self.deps.notifiers.send(&notifier_cfg, &rendered).await {
            Ok(()) => Ok(ChannelTestResult {
                channel_id: channel.id,
                channel_type: ch_type,
                status: DeliveryStatus::Sent,
                error_message: None,
                sent_at,
            }),
            Err(err @ CoreError::SsrfBlocked(_)) => Err(err),
            Err(err) => Ok(ChannelTestResult {
                channel_id: channel.id,
                channel_type: ch_type,
                status: DeliveryStatus::Failed,
                error_message: Some(err.to_string()),
                sent_at,
            }),
        }
    }

    async fn resend_delivery(
        &self,
        user_id: Uuid,
        delivery_id: Uuid,
    ) -> Result<DeliveryLogRecord, CoreError> {
        let delivery = self
            .deps
            .repos
            .runs
            .get_delivery_by_id(user_id, delivery_id)
            .await?
            .ok_or_else(|| CoreError::NotFound(format!("delivery log {delivery_id} not found")))?;

        let digest_id = delivery.digest_id.ok_or_else(|| {
            CoreError::InvalidInput(
                "cannot resend delivery without an associated daily digest".to_owned(),
            )
        })?;
        let channel_id = delivery.channel_id.ok_or_else(|| {
            CoreError::InvalidInput(
                "cannot resend delivery without an associated notification channel".to_owned(),
            )
        })?;

        let digest = self.find_digest_by_id(user_id, digest_id).await?;
        let channel = self
            .deps
            .repos
            .settings
            .get_channel(user_id, channel_id)
            .await?
            .ok_or_else(|| {
                CoreError::NotFound(format!("notification channel {channel_id} not found"))
            })?;

        let cipher = Arc::clone(&self.deps.cipher);
        let config_enc = channel.config_enc.clone();
        let config_json = tokio::task::spawn_blocking(move || {
            let plain = cipher.decrypt(&config_enc)?;
            serde_json::from_str::<serde_json::Value>(&plain).map_err(|err| {
                CoreError::Crypto(format!("invalid decrypted channel config JSON: {err}"))
            })
        })
        .await
        .map_err(|err| CoreError::Internal(format!("channel decrypt task failed: {err}")))??;

        let rendered =
            render_digest_detail_blocking(Arc::clone(&self.deps.renderer), &digest).await?;

        let notifier_cfg = NotifierConfig {
            channel_id: Some(channel.id),
            channel_type: channel.channel_type,
            name: channel.name,
            config_json,
        };

        let sent_at = Utc::now();
        let (status, error_message) = match self.deps.notifiers.send(&notifier_cfg, &rendered).await
        {
            Ok(()) => (DeliveryStatus::Sent, None),
            Err(err @ CoreError::SsrfBlocked(_)) => return Err(err),
            Err(err) => (DeliveryStatus::Failed, Some(err.to_string())),
        };

        self.deps
            .repos
            .runs
            .update_delivery_status(user_id, delivery_id, status, error_message, sent_at)
            .await
    }

    async fn test_platform_x_descriptor(
        &self,
        descriptor: SourceDescriptor,
        platform_x: &EffectivePlatformXConfig,
    ) -> Result<digest_core::traits::FetchResult, CoreError> {
        fetch_with_platform_x_support(
            &self.deps.repos,
            &self.deps.cipher,
            &self.deps.collectors,
            &descriptor,
            &ConditionalFetchHeaders::default(),
            None,
            Some(platform_x),
            self.deps.config.platform_x_bridge_base_url.as_deref(),
        )
        .await
    }
}

fn sanitize_secret_from_msg(msg: &str, secret: Option<&str>) -> String {
    match secret.map(str::trim).filter(|s| !s.is_empty()) {
        Some(secret_val) => msg.replace(secret_val, SECRET_MASK_REDACTED_PLACEHOLDER),
        None => msg.to_owned(),
    }
}

async fn render_channel_test_notification_blocking(
    renderer: Arc<RendererRegistry>,
    channel_name: &str,
) -> Result<RenderedNotification, CoreError> {
    let today = Utc::now().date_naive();
    let render_input = RenderInput {
        digest_date: today,
        digest: SummarizedDigest {
            title: format!("Gemini Daily Digest — Test ({channel_name})"),
            overview_markdown:
                "This is a test notification verifying your Gemini Daily Digest channel configuration."
                    .to_owned(),
            prompt_tokens: 0,
            completion_tokens: 0,
            model_used: TEST_RENDER_MODEL_LABEL.to_owned(),
            degraded_fallback: false,
        },
        sections: Vec::new(),
        highlighted_items: Vec::new(),
        all_items: Vec::new(),
        priority: AlertPriority::Normal,
        locale: digest_core::domain::default_digest_locale(),
    };

    tokio::task::spawn_blocking(move || renderer.render_structured(&render_input))
        .await
        .map_err(|err| CoreError::Internal(format!("test notification render failed: {err}")))?
}

async fn render_digest_detail_blocking(
    renderer: Arc<RendererRegistry>,
    digest: &DailyDigestDetail,
) -> Result<RenderedNotification, CoreError> {
    let mut all_items: Vec<ProcessedItem> = Vec::new();
    for section in &digest.sections {
        all_items.extend(section.items.iter().cloned());
    }
    let highlighted_items: Vec<ProcessedItem> = all_items
        .iter()
        .filter(|item| {
            matches!(
                item.match_result.priority,
                AlertPriority::High | AlertPriority::Urgent
            )
        })
        .cloned()
        .collect();
    let highest_priority = all_items
        .iter()
        .map(|item| item.match_result.priority)
        .max_by_key(|priority| priority_rank(*priority))
        .unwrap_or(AlertPriority::Normal);

    let render_input = RenderInput {
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
        highlighted_items,
        all_items,
        priority: highest_priority,
        locale: digest_core::domain::default_digest_locale(),
    };

    tokio::task::spawn_blocking(move || renderer.render_structured(&render_input))
        .await
        .map_err(|err| CoreError::Internal(format!("digest render task failed: {err}")))?
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use digest_core::cache::{CacheCoordinator, InMemoryCacheCoordinator};
    use digest_core::collectors::{CollectorContext, CollectorRegistry};
    use digest_core::config::{CollectorTuning, LlmTuning, NotifierTuning};
    use digest_core::crypto::EnvelopeCipher;
    use digest_core::domain::{NotificationChannelType, SourceScope, SourceType, SseEventType};
    use digest_core::http::shared_http_client;
    use digest_core::llm::IdempotentLlmRunner;
    use digest_core::notifiers::NotifierRegistry;
    use digest_core::pipeline::ItemProcessingPipeline;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;
    use crate::config::ServerConfig;
    use crate::repo::{
        encode_source_config, CreateChannelRecord, CreateDeliveryLogCmd, SaveDigestCmd,
        SubscribeSourceCmd, UpsertAiConfigRecord,
    };
    use crate::scheduler::test_support::MockRepos;
    use crate::sse_hub::SseHub;

    fn build_orchestrator(mock: &MockRepos, allow_local: bool) -> ServerExecutionOrchestrator {
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
        if allow_local {
            cfg.core.ssrf_allowlist_hosts = vec!["127.0.0.1".to_owned(), "localhost".to_owned()];
        }

        let client = shared_http_client(None, &cfg.core.collector).unwrap();
        let ssrf = SsrfGuard::from_config(&cfg.core);
        let cache: Arc<dyn CacheCoordinator> = Arc::new(InMemoryCacheCoordinator::new());
        let collectors = Arc::new(CollectorRegistry::new(CollectorContext::new(
            client.clone(),
            ssrf.clone(),
            cfg.core.collector.clone(),
            cfg.core.resources.clone(),
        )));
        let provider: Arc<dyn StructuredLlmProvider> = Arc::new(OpenAiCompatProvider::with_tuning(
            client.clone(),
            ssrf.clone(),
            cfg.core.llm.clone(),
            cfg.core.resources.max_feed_bytes.get(),
        ));
        let llm_runner = Arc::new(IdempotentLlmRunner::new(
            Arc::clone(&cache),
            Arc::clone(&provider),
        ));
        let notifiers = Arc::new(NotifierRegistry::with_defaults(
            client,
            ssrf.clone(),
            NotifierTuning::default(),
        ));

        let deps = ComposerDeps {
            config: Arc::new(cfg),
            repos: mock.repositories(),
            cipher: Arc::new(EnvelopeCipher::from_bytes([0x44u8; 32])),
            cache,
            collectors,
            pipeline: Arc::new(ItemProcessingPipeline::new()),
            llm_runner,
            renderer: Arc::new(RendererRegistry::new()),
            notifiers,
            sse: Arc::new(SseHub::new()),
        };

        ServerExecutionOrchestrator::new(deps, ssrf, provider)
    }

    #[tokio::test]
    async fn test_ai_config_validates_ssrf_and_probes_provider_without_leaking_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer test-secret-probe-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "deepseek-chat",
                "choices": [{ "message": { "content": "OK" } }],
                "usage": { "prompt_tokens": 5, "completion_tokens": 1 }
            })))
            .mount(&server)
            .await;

        let mock = MockRepos::new();
        let orch = build_orchestrator(&mock, true);
        let user_id = Uuid::new_v4();

        let enc = orch.deps.cipher.encrypt("test-secret-probe-key").unwrap();
        orch.deps
            .repos
            .settings
            .upsert_ai_config(
                user_id,
                UpsertAiConfigRecord {
                    provider_label: "DeepSeek".to_owned(),
                    base_url: server.uri(),
                    api_key_enc: Some(enc),
                    api_key_hint: Some("test****-key".to_owned()),
                    model: "deepseek-chat".to_owned(),
                    system_prompt: "Test".to_owned(),
                    temperature: 0.2,
                    max_tokens: 1024,
                },
            )
            .await
            .unwrap();

        let ok_res = orch
            .test_ai_config(user_id, AiTestRequest::default())
            .await
            .unwrap();
        assert!(ok_res.ok);
        assert_eq!(ok_res.model, "deepseek-chat");
        assert!(!ok_res.message.contains("test-secret-probe-key"));

        // SSRF check rejects metadata IP when not allowlisted.
        let strict_orch = build_orchestrator(&mock, false);
        let ssrf_err = strict_orch
            .test_ai_config(
                user_id,
                AiTestRequest {
                    base_url: Some("http://169.254.169.254/v1".to_owned()),
                    ..AiTestRequest::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(ssrf_err, CoreError::SsrfBlocked(_)));
    }

    #[tokio::test]
    async fn list_ai_models_validates_ssrf_uses_draft_and_stored_keys_and_redacts_secrets() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer stored-models-secret-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [
                    { "id": "deepseek-reasoner", "owned_by": "deepseek" },
                    { "id": "deepseek-chat", "owned_by": "deepseek" }
                ]
            })))
            .mount(&server)
            .await;

        let mock = MockRepos::new();
        let orch = build_orchestrator(&mock, true);
        let user_id = Uuid::new_v4();

        let enc = orch
            .deps
            .cipher
            .encrypt("stored-models-secret-key")
            .unwrap();
        orch.deps
            .repos
            .settings
            .upsert_ai_config(
                user_id,
                UpsertAiConfigRecord {
                    provider_label: "DeepSeek".to_owned(),
                    base_url: server.uri(),
                    api_key_enc: Some(enc),
                    api_key_hint: Some("stor****-key".to_owned()),
                    model: "deepseek-chat".to_owned(),
                    system_prompt: "Test".to_owned(),
                    temperature: 0.2,
                    max_tokens: 1024,
                },
            )
            .await
            .unwrap();

        let stored_res = orch
            .list_ai_models(user_id, AiModelsRequest::default())
            .await
            .unwrap();
        assert_eq!(stored_res.models.len(), 2);
        assert_eq!(stored_res.models[0].id, "deepseek-chat");
        assert_eq!(stored_res.models[1].id, "deepseek-reasoner");

        // Error response echoes draft key -> must be sanitized before returning.
        Mock::given(method("GET"))
            .and(path("/custom/v1/models"))
            .respond_with(
                ResponseTemplate::new(401).set_body_string("bad key: draft-secret-override-key"),
            )
            .mount(&server)
            .await;

        let err = orch
            .list_ai_models(
                user_id,
                AiModelsRequest {
                    base_url: Some(format!("{}/custom/v1", server.uri())),
                    api_key: Some("draft-secret-override-key".to_owned()),
                    clear_api_key: false,
                },
            )
            .await
            .unwrap_err();
        let err_text = err.to_string();
        assert!(!err_text.contains("draft-secret-override-key"));

        // SSRF check blocks internal addresses.
        let strict_orch = build_orchestrator(&mock, false);
        let ssrf_err = strict_orch
            .list_ai_models(
                user_id,
                AiModelsRequest {
                    base_url: Some("http://169.254.169.254/v1".to_owned()),
                    ..AiModelsRequest::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(ssrf_err, CoreError::SsrfBlocked(_)));
    }

    #[tokio::test]
    async fn test_source_and_test_channel_and_resend_delivery_work_end_to_end() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/preview.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Preview</title><item><guid>1</guid><title>Gemini Item One</title><link>https://example.com/1</link></item></channel></rss>"#,
            ))
            .mount(&server)
            .await;

        let hook_hits = Arc::new(AtomicUsize::new(0));
        let hook_hits_clone = Arc::clone(&hook_hits);
        Mock::given(method("POST"))
            .and(path("/webhook"))
            .respond_with(move |_req: &Request| {
                hook_hits_clone.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
            })
            .mount(&server)
            .await;

        let mock = MockRepos::new();
        let orch = build_orchestrator(&mock, true);
        let user_id = Uuid::new_v4();

        let src_enc = encode_source_config(
            &orch.deps.cipher,
            &format!("{}/preview.xml", server.uri()),
            None,
        )
        .unwrap();
        let src = orch
            .deps
            .repos
            .sources
            .create_or_subscribe(
                user_id,
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "preview-rss".to_owned(),
                    custom_name: "Preview".to_owned(),
                    default_name: "Preview".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: format!("{}/preview.xml", server.uri()),
                    config_json_enc: Some(src_enc),
                    enabled: true,
                },
            )
            .await
            .unwrap();

        let src_test = orch.test_source(user_id, src.id).await.unwrap();
        assert_eq!(src_test.outcome, FetchOutcome::Updated);
        assert_eq!(src_test.items_fetched, 1);
        assert_eq!(src_test.sample_titles, vec!["Gemini Item One".to_owned()]);

        let ch_enc = orch
            .deps
            .cipher
            .encrypt(&serde_json::json!({ "url": format!("{}/webhook", server.uri()) }).to_string())
            .unwrap();
        let ch = orch
            .deps
            .repos
            .settings
            .create_channel(
                user_id,
                CreateChannelRecord {
                    channel_type: NotificationChannelType::Webhook,
                    name: "Test Webhook".to_owned(),
                    enabled: true,
                    config_enc: ch_enc,
                    config_hint: "https://example.com".to_owned(),
                    trigger_on_daily: true,
                    trigger_on_alert: true,
                },
            )
            .await
            .unwrap();

        let ch_test = orch.test_channel(user_id, ch.id).await.unwrap();
        assert_eq!(ch_test.status, DeliveryStatus::Sent);
        assert_eq!(hook_hits.load(Ordering::SeqCst), 1);

        // Save digest + failed delivery log, then resend_delivery succeeds and updates status to Sent.
        let digest = orch
            .deps
            .repos
            .digests
            .save_daily_and_source_digests(SaveDigestCmd {
                user_id,
                digest_date: NaiveDate::from_ymd_opt(2026, 11, 26).unwrap(),
                input_fingerprint: "fp-resend".to_owned(),
                title: "Resend Digest".to_owned(),
                overview_markdown: "Overview".to_owned(),
                overview_html: "<p>Overview</p>".to_owned(),
                sections: Vec::new(),
                item_count: 1,
                highlighted_count: 0,
                prompt_tokens: 0,
                completion_tokens: 0,
                model_used: "deepseek-chat".to_owned(),
                degraded_fallback: false,
                source_digests: Vec::new(),
            })
            .await
            .unwrap();

        let run = orch
            .deps
            .repos
            .runs
            .create_run(CreateRunCmd {
                user_id,
                run_type: RunType::ManualDaily,
                started_at: Utc::now(),
            })
            .await
            .unwrap();
        let failed_delivery = orch
            .deps
            .repos
            .runs
            .record_delivery(CreateDeliveryLogCmd {
                run_id: run.id,
                user_id,
                digest_id: Some(digest.id),
                channel_id: Some(ch.id),
                channel_type: NotificationChannelType::Webhook,
                status: DeliveryStatus::Failed,
                error_message: Some("timeout".to_owned()),
                sent_at: Utc::now(),
            })
            .await
            .unwrap();

        let resent = orch
            .resend_delivery(user_id, failed_delivery.id)
            .await
            .unwrap();
        assert_eq!(resent.status, DeliveryStatus::Sent);
        assert!(resent.error_message.is_none());
        assert_eq!(hook_hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn trigger_manual_run_returns_running_immediately_and_streams_sse_completion() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/manual.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Manual</title><item><guid>m1</guid><title>Manual Run Item</title><link>https://example.com/m1</link></item></channel></rss>"#,
            ))
            .mount(&server)
            .await;

        let mock = MockRepos::new();
        let orch = build_orchestrator(&mock, true);
        let user_id = Uuid::new_v4();
        let mut sse_rx = orch.deps.sse.subscribe(user_id);

        let src_enc = encode_source_config(
            &orch.deps.cipher,
            &format!("{}/manual.xml", server.uri()),
            None,
        )
        .unwrap();
        orch.deps
            .repos
            .sources
            .create_or_subscribe(
                user_id,
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "manual-rss".to_owned(),
                    custom_name: "Manual".to_owned(),
                    default_name: "Manual".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: format!("{}/manual.xml", server.uri()),
                    config_json_enc: Some(src_enc),
                    enabled: true,
                },
            )
            .await
            .unwrap();

        let initial = orch
            .trigger_manual_run(
                user_id,
                TriggerRunRequest {
                    digest_date: NaiveDate::from_ymd_opt(2026, 11, 26),
                    force_refresh: true,
                    notify: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(initial.status, RunStatus::Running);

        // Wait for background run to emit DigestReady over SSE.
        let mut saw_ready = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(env)) =
                tokio::time::timeout(Duration::from_millis(200), sse_rx.recv()).await
            {
                if env.event_type == SseEventType::DigestReady {
                    saw_ready = true;
                    break;
                }
            }
        }
        assert!(
            saw_ready,
            "expected DigestReady SSE event from spawned manual run"
        );
    }
}
