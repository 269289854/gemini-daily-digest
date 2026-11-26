//! Pure deterministic fallback summarizer with zero network calls or token cost.

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::constants::FALLBACK_MODEL_LABEL;
use crate::domain::{
    AlertPriority, DailyDigestSynthesisInput, ProcessedItem, SourceDigestInput,
    SourceSummaryResult, UserAiConfig,
};
use crate::error::CoreError;
use crate::llm::fingerprint::{compute_input_fingerprint, compute_source_input_fingerprint};
use crate::traits::{LlmProvider, RawCollectedItem, StructuredLlmProvider, SummarizedDigest};

const MAX_FALLBACK_ITEMS_PER_SECTION: usize = 12;
const MAX_FALLBACK_KEY_POINTS: usize = 5;

/// Deterministic summarizer used when no LLM key is configured or the upstream LLM fails.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeterministicFallbackSummarizer;

impl DeterministicFallbackSummarizer {
    /// Creates a new deterministic fallback summarizer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl LlmProvider for DeterministicFallbackSummarizer {
    async fn summarize(
        &self,
        _system_prompt: &str,
        items: &[RawCollectedItem],
    ) -> Result<SummarizedDigest, CoreError> {
        let overview_markdown = if items.is_empty() {
            "No new updates were collected for this digest window.".to_owned()
        } else {
            let mut lines = Vec::with_capacity(items.len() + 2);
            lines.push(format!(
                "Collected {} update(s) (deterministic fallback summary):",
                items.len()
            ));
            lines.push(String::new());
            for item in items.iter().take(MAX_FALLBACK_ITEMS_PER_SECTION) {
                lines.push(format!("- [{}]({})", item.title.trim(), item.url.trim()));
            }
            if items.len() > MAX_FALLBACK_ITEMS_PER_SECTION {
                lines.push(format!(
                    "- ...and {} more item(s).",
                    items.len() - MAX_FALLBACK_ITEMS_PER_SECTION
                ));
            }
            lines.join("\n")
        };

        Ok(SummarizedDigest {
            title: "Gemini Ecosystem Digest (Fallback)".to_owned(),
            overview_markdown,
            prompt_tokens: 0,
            completion_tokens: 0,
            model_used: FALLBACK_MODEL_LABEL.to_owned(),
            degraded_fallback: true,
        })
    }
}

#[async_trait]
impl StructuredLlmProvider for DeterministicFallbackSummarizer {
    async fn summarize_source(
        &self,
        config: &UserAiConfig,
        input: &SourceDigestInput,
    ) -> Result<SourceSummaryResult, CoreError> {
        let fingerprint = compute_source_input_fingerprint(
            input.digest_date,
            input.source.id,
            config,
            &input.items,
        );

        let mut sorted_items = input.items.clone();
        sort_items_for_digest(&mut sorted_items);

        let key_points: Vec<String> = sorted_items
            .iter()
            .take(MAX_FALLBACK_KEY_POINTS)
            .map(format_key_point)
            .collect();

        let summary_markdown = if sorted_items.is_empty() {
            format!(
                "No new items collected from **{}** on {}.",
                input.source.name, input.digest_date
            )
        } else {
            let mut lines = Vec::new();
            lines.push(format!(
                "**{}** ({}) — {} item(s) collected:",
                input.source.name,
                input.source.category,
                sorted_items.len()
            ));
            for item in sorted_items.iter().take(MAX_FALLBACK_ITEMS_PER_SECTION) {
                lines.push(format_markdown_bullet(item));
            }
            if sorted_items.len() > MAX_FALLBACK_ITEMS_PER_SECTION {
                lines.push(format!(
                    "- ...and {} additional item(s).",
                    sorted_items.len() - MAX_FALLBACK_ITEMS_PER_SECTION
                ));
            }
            lines.join("\n")
        };

        Ok(SourceSummaryResult {
            source_id: input.source.id,
            source_name: input.source.name.clone(),
            category: input.source.category.clone(),
            digest_date: input.digest_date,
            input_fingerprint: fingerprint,
            summary_markdown,
            key_points,
            item_count: input.items.len(),
            prompt_tokens: 0,
            completion_tokens: 0,
            model_used: FALLBACK_MODEL_LABEL.to_owned(),
            degraded_fallback: true,
        })
    }

    async fn synthesize_daily(
        &self,
        config: &UserAiConfig,
        input: &DailyDigestSynthesisInput,
    ) -> Result<SummarizedDigest, CoreError> {
        let _fp = compute_input_fingerprint(input.digest_date, config, &input.all_items);
        let mut lines = Vec::new();
        lines.push(format!(
            "Deterministic summary for **{}** ({} total item(s), {} highlighted alert(s)).",
            input.digest_date,
            input.all_items.len(),
            input.highlighted_items.len()
        ));

        if !input.highlighted_items.is_empty() {
            lines.push(String::new());
            lines.push("### Watchlist Alerts".to_owned());
            let mut alerts = input.highlighted_items.clone();
            sort_items_for_digest(&mut alerts);
            for item in alerts.iter().take(MAX_FALLBACK_ITEMS_PER_SECTION) {
                lines.push(format_markdown_bullet(item));
            }
        }

        if !input.source_summaries.is_empty() {
            lines.push(String::new());
            lines.push("### Source Highlights".to_owned());
            let mut by_category: BTreeMap<&str, Vec<&SourceSummaryResult>> = BTreeMap::new();
            for summary in &input.source_summaries {
                by_category
                    .entry(summary.category.as_str())
                    .or_default()
                    .push(summary);
            }
            for (category, summaries) in by_category {
                lines.push(format!("- **{category}**:"));
                for summary in summaries {
                    let first_point = summary
                        .key_points
                        .first()
                        .map(String::as_str)
                        .unwrap_or("No highlights");
                    lines.push(format!(
                        "  - {} ({} item(s)): {}",
                        summary.source_name, summary.item_count, first_point
                    ));
                }
            }
        } else if !input.all_items.is_empty() {
            lines.push(String::new());
            lines.push("### Top Updates".to_owned());
            let mut top_items = input.all_items.clone();
            sort_items_for_digest(&mut top_items);
            for item in top_items.iter().take(MAX_FALLBACK_ITEMS_PER_SECTION) {
                lines.push(format_markdown_bullet(item));
            }
        }

        Ok(SummarizedDigest {
            title: format!("Gemini Daily Digest — {}", input.digest_date),
            overview_markdown: lines.join("\n"),
            prompt_tokens: 0,
            completion_tokens: 0,
            model_used: FALLBACK_MODEL_LABEL.to_owned(),
            degraded_fallback: true,
        })
    }

    async fn test_connection(&self, _config: &UserAiConfig) -> Result<(), CoreError> {
        Ok(())
    }
}

fn priority_rank(priority: AlertPriority) -> u8 {
    match priority {
        AlertPriority::Urgent => 3,
        AlertPriority::High => 2,
        AlertPriority::Normal => 1,
    }
}

fn sort_items_for_digest(items: &mut [ProcessedItem]) {
    items.sort_by(|a, b| {
        priority_rank(b.match_result.priority)
            .cmp(&priority_rank(a.match_result.priority))
            .then_with(|| {
                b.match_result
                    .importance_score
                    .cmp(&a.match_result.importance_score)
            })
            .then_with(|| b.published_at.cmp(&a.published_at))
            .then_with(|| a.title.cmp(&b.title))
    });
}

fn format_key_point(item: &ProcessedItem) -> String {
    if item.match_result.matched_keywords.is_empty() {
        format!("{} ({})", item.title.trim(), item.canonical_url.trim())
    } else {
        format!(
            "[{}] {} [{}] ({})",
            item.match_result.priority.as_str().to_uppercase(),
            item.title.trim(),
            item.match_result.matched_keywords.join(", "),
            item.canonical_url.trim()
        )
    }
}

fn format_markdown_bullet(item: &ProcessedItem) -> String {
    let prefix = match item.match_result.priority {
        AlertPriority::Urgent => "[URGENT] ",
        AlertPriority::High => "[HIGH] ",
        AlertPriority::Normal => "",
    };
    let keywords = if item.match_result.matched_keywords.is_empty() {
        String::new()
    } else {
        format!(
            " _(matched: {})_",
            item.match_result.matched_keywords.join(", ")
        )
    };
    format!(
        "- {prefix}[{}]({}){keywords}",
        item.title.trim(),
        item.canonical_url.trim()
    )
}
