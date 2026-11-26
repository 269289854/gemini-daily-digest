//! Deterministic input fingerprinting and cache-backed idempotent LLM execution.

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::cache::CacheCoordinator;
use crate::constants::{CACHE_DIGEST_FINGERPRINT_NAMESPACE, DEFAULT_DIGEST_FINGERPRINT_TTL_SECS};
use crate::domain::{
    normalize_digest_locale, DailyDigestSynthesisInput, ProcessedItem, SourceDigestInput,
    SourceSummaryResult, UserAiConfig,
};
use crate::error::CoreError;
use crate::llm::fallback::DeterministicFallbackSummarizer;
use crate::traits::{StructuredLlmProvider, SummarizedDigest};

/// Computes a deterministic SHA-256 hex fingerprint over digest date, model,
/// system prompt, and sorted item keys (`canonical_url`, `content_hash`, `title`).
///
/// Secrets such as `ai_config.api_key` are NEVER included in the fingerprint.
#[must_use]
pub fn compute_input_fingerprint(
    digest_date: NaiveDate,
    ai_config: &UserAiConfig,
    items: &[ProcessedItem],
) -> String {
    compute_fingerprint_with_scope(None, digest_date, ai_config, items)
}

/// Computes a deterministic SHA-256 hex fingerprint for a single source's items.
#[must_use]
pub fn compute_source_input_fingerprint(
    digest_date: NaiveDate,
    source_id: Uuid,
    ai_config: &UserAiConfig,
    items: &[ProcessedItem],
) -> String {
    compute_fingerprint_with_scope(Some(source_id), digest_date, ai_config, items)
}

fn compute_fingerprint_with_scope(
    source_id: Option<Uuid>,
    digest_date: NaiveDate,
    ai_config: &UserAiConfig,
    items: &[ProcessedItem],
) -> String {
    let mut keys: Vec<(&str, &str, &str)> = items
        .iter()
        .map(|item| {
            (
                item.canonical_url.as_str(),
                item.content_hash.as_str(),
                item.title.as_str(),
            )
        })
        .collect();
    keys.sort_unstable();

    let mut hasher = Sha256::new();
    hash_field(&mut hasher, b"date", digest_date.to_string().as_bytes());
    if let Some(id) = source_id {
        hash_field(&mut hasher, b"source_id", id.to_string().as_bytes());
    }
    hash_field(&mut hasher, b"model", ai_config.model.trim().as_bytes());
    hash_field(
        &mut hasher,
        b"system_prompt",
        ai_config.system_prompt.trim().as_bytes(),
    );
    hash_field(
        &mut hasher,
        b"output_language",
        normalize_digest_locale(&ai_config.output_language).as_bytes(),
    );

    let count_bytes = (keys.len() as u64).to_le_bytes();
    hash_field(&mut hasher, b"item_count", &count_bytes);

    for (url, content_hash, title) in keys {
        hash_field(&mut hasher, b"url", url.as_bytes());
        hash_field(&mut hasher, b"content_hash", content_hash.as_bytes());
        hash_field(&mut hasher, b"title", title.as_bytes());
    }

    hex::encode(hasher.finalize())
}

fn hash_field(hasher: &mut Sha256, label: &[u8], value: &[u8]) {
    hasher.update((label.len() as u64).to_le_bytes());
    hasher.update(label);
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

/// Result returned by `IdempotentLlmRunner::summarize_daily_with_metadata`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdempotentDigestResult {
    pub digest: SummarizedDigest,
    pub input_fingerprint: String,
    pub cache_hit: bool,
}

/// Runs LLM summarization idempotently using `CacheCoordinator` keyed by `input_fingerprint`,
/// automatically falling back to `DeterministicFallbackSummarizer` on primary failure or timeout.
pub struct IdempotentLlmRunner {
    cache: Arc<dyn CacheCoordinator>,
    primary: Arc<dyn StructuredLlmProvider>,
    fallback: DeterministicFallbackSummarizer,
    ttl: Duration,
}

impl IdempotentLlmRunner {
    /// Creates an `IdempotentLlmRunner` using `DEFAULT_DIGEST_FINGERPRINT_TTL_SECS`.
    #[must_use]
    pub fn new(cache: Arc<dyn CacheCoordinator>, primary: Arc<dyn StructuredLlmProvider>) -> Self {
        Self::with_ttl(
            cache,
            primary,
            Duration::from_secs(DEFAULT_DIGEST_FINGERPRINT_TTL_SECS),
        )
    }

    /// Creates an `IdempotentLlmRunner` with an explicit cache TTL.
    #[must_use]
    pub fn with_ttl(
        cache: Arc<dyn CacheCoordinator>,
        primary: Arc<dyn StructuredLlmProvider>,
        ttl: Duration,
    ) -> Self {
        Self {
            cache,
            primary,
            fallback: DeterministicFallbackSummarizer::new(),
            ttl,
        }
    }

    /// Summarizes a daily digest idempotently; cache hits return zero token usage.
    pub async fn summarize_daily_idempotent(
        &self,
        ai_config: &UserAiConfig,
        input: &DailyDigestSynthesisInput,
    ) -> Result<SummarizedDigest, CoreError> {
        let outcome = self
            .summarize_daily_with_options(ai_config, input, false)
            .await?;
        Ok(outcome.digest)
    }

    /// Summarizes a daily digest and returns fingerprint and cache-hit metadata.
    pub async fn summarize_daily_with_options(
        &self,
        ai_config: &UserAiConfig,
        input: &DailyDigestSynthesisInput,
        force_refresh: bool,
    ) -> Result<IdempotentDigestResult, CoreError> {
        let input_fingerprint =
            compute_input_fingerprint(input.digest_date, ai_config, &input.all_items);
        let cache_key = format!("{CACHE_DIGEST_FINGERPRINT_NAMESPACE}{input_fingerprint}");

        if !force_refresh {
            match self.cache.get_cached_value(&cache_key).await {
                Ok(Some(raw_json)) => {
                    if let Ok(mut cached) = serde_json::from_str::<SummarizedDigest>(&raw_json) {
                        cached.prompt_tokens = 0;
                        cached.completion_tokens = 0;
                        return Ok(IdempotentDigestResult {
                            digest: cached,
                            input_fingerprint,
                            cache_hit: true,
                        });
                    }
                    tracing::warn!(
                        cache_key = %cache_key,
                        "discarding unreadable cached daily digest entry"
                    );
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "cache lookup failed for daily digest; proceeding without cache"
                    );
                }
            }
        }

        let digest = match self.primary.synthesize_daily(ai_config, input).await {
            Ok(primary_digest) => {
                if !primary_digest.degraded_fallback {
                    match serde_json::to_string(&primary_digest) {
                        Ok(serialized) => {
                            if let Err(err) = self
                                .cache
                                .set_cached_value(&cache_key, &serialized, self.ttl)
                                .await
                            {
                                tracing::warn!(
                                    error = %err,
                                    "failed to cache daily digest result"
                                );
                            }
                        }
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                "failed to serialize daily digest for cache"
                            );
                        }
                    }
                }
                primary_digest
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "primary LLM daily synthesis failed; falling back to deterministic summarizer"
                );
                self.fallback.synthesize_daily(ai_config, input).await?
            }
        };

        Ok(IdempotentDigestResult {
            digest,
            input_fingerprint,
            cache_hit: false,
        })
    }

    /// Summarizes a single source idempotently with deterministic fallback on primary failure.
    pub async fn summarize_source_idempotent(
        &self,
        ai_config: &UserAiConfig,
        input: &SourceDigestInput,
        force_refresh: bool,
    ) -> Result<SourceSummaryResult, CoreError> {
        let fingerprint = compute_source_input_fingerprint(
            input.digest_date,
            input.source.id,
            ai_config,
            &input.items,
        );
        let cache_key = format!("{CACHE_DIGEST_FINGERPRINT_NAMESPACE}src:{fingerprint}");

        if !force_refresh {
            if let Ok(Some(raw_json)) = self.cache.get_cached_value(&cache_key).await {
                if let Ok(mut cached) = serde_json::from_str::<SourceSummaryResult>(&raw_json) {
                    cached.prompt_tokens = 0;
                    cached.completion_tokens = 0;
                    return Ok(cached);
                }
            }
        }

        match self.primary.summarize_source(ai_config, input).await {
            Ok(summary) => {
                if !summary.degraded_fallback {
                    if let Ok(serialized) = serde_json::to_string(&summary) {
                        if let Err(err) = self
                            .cache
                            .set_cached_value(&cache_key, &serialized, self.ttl)
                            .await
                        {
                            tracing::warn!(
                                error = %err,
                                "failed to cache source digest summary"
                            );
                        }
                    }
                }
                Ok(summary)
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    source_id = %input.source.id,
                    "primary LLM source summary failed; falling back to deterministic summarizer"
                );
                self.fallback.summarize_source(ai_config, input).await
            }
        }
    }
}
