//! `ItemProcessingPipeline`: URL normalization with tracking parameter stripping,
//! SHA-256 content hashing & deduplication, watchlist matching, and importance scoring.

pub mod dedup;
pub mod normalize;
pub mod scoring;
pub mod watchlist;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::domain::{ItemMatchResult, ProcessedItem, WatchlistRuleInput};
use crate::error::CoreError;
use crate::traits::{ItemProcessor, RawCollectedItem, SourceDescriptor, StructuredItemProcessor};

pub use dedup::{compute_content_hash, deduplicate_processed_items, normalize_text_for_hash};
pub use normalize::normalize_url;
pub use scoring::{score_item_at, sort_processed_items, ScoringWeights};
pub use watchlist::{match_watchlist, priority_rank, WatchlistMatch};

/// Deterministic item processing pipeline implementing `ItemProcessor` and `StructuredItemProcessor`.
#[derive(Debug, Clone, Default)]
pub struct ItemProcessingPipeline {
    weights: ScoringWeights,
}

impl ItemProcessingPipeline {
    /// Creates an `ItemProcessingPipeline` with default scoring weights from `constants`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an `ItemProcessingPipeline` with custom scoring weights.
    #[must_use]
    pub fn with_weights(weights: ScoringWeights) -> Self {
        Self { weights }
    }

    /// Synchronously processes a batch of raw items using explicit `now` timestamp.
    pub fn process_items_sync(
        &self,
        source: Option<&SourceDescriptor>,
        items: Vec<RawCollectedItem>,
        watchlist_rules: &[WatchlistRuleInput],
        now: DateTime<Utc>,
    ) -> Result<Vec<ProcessedItem>, CoreError> {
        let mut processed = Vec::with_capacity(items.len());

        for raw in items {
            let canonical_url = normalize_url(&raw.url)?;
            let clean_title = raw.title.trim().to_owned();
            let title = if clean_title.is_empty() {
                canonical_url.clone()
            } else {
                clean_title
            };
            let author = raw
                .author
                .map(|a| a.trim().to_owned())
                .filter(|a| !a.is_empty());
            let summary = raw
                .summary
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());

            let content_hash = compute_content_hash(&canonical_url, &title, summary.as_deref());
            let wl = match_watchlist(
                &title,
                summary.as_deref(),
                author.as_deref(),
                watchlist_rules,
            );
            let importance_score = score_item_at(
                raw.published_at,
                summary.as_deref(),
                &wl.matched_keywords,
                wl.priority,
                now,
                &self.weights,
            );

            processed.push(ProcessedItem {
                external_id: raw
                    .external_id
                    .map(|id| id.trim().to_owned())
                    .filter(|id| !id.is_empty()),
                canonical_url,
                content_hash,
                title,
                author,
                summary,
                published_at: raw.published_at,
                source_id: source.map(|s| s.id),
                source_name: source.map(|s| s.name.clone()),
                category: source.map(|s| s.category.clone()),
                match_result: ItemMatchResult {
                    matched_keywords: wl.matched_keywords,
                    priority: wl.priority,
                    importance_score,
                    notify_immediately: wl.notify_immediately,
                },
            });
        }

        let mut deduped = deduplicate_processed_items(processed);
        sort_processed_items(&mut deduped);
        Ok(deduped)
    }
}

#[async_trait]
impl ItemProcessor for ItemProcessingPipeline {
    async fn process(
        &self,
        items: Vec<RawCollectedItem>,
    ) -> Result<Vec<RawCollectedItem>, CoreError> {
        let processed = self.process_items(None, items, &[]).await?;
        Ok(processed.iter().map(ProcessedItem::to_raw_item).collect())
    }
}

#[async_trait]
impl StructuredItemProcessor for ItemProcessingPipeline {
    async fn process_items(
        &self,
        source: Option<&SourceDescriptor>,
        items: Vec<RawCollectedItem>,
        watchlist_rules: &[WatchlistRuleInput],
    ) -> Result<Vec<ProcessedItem>, CoreError> {
        let pipeline = self.clone();
        let source_owned = source.cloned();
        let rules_owned = watchlist_rules.to_vec();
        let now = Utc::now();

        tokio::task::spawn_blocking(move || {
            pipeline.process_items_sync(source_owned.as_ref(), items, &rules_owned, now)
        })
        .await
        .map_err(|err| CoreError::Internal(format!("item processing task failed: {err}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{AlertPriority, SourceScope, SourceType};
    use chrono::Duration;
    use uuid::Uuid;

    #[tokio::test]
    async fn pipeline_normalizes_dedups_matches_and_orders_items() {
        let now = Utc::now();
        let source = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::Rss,
            scope: SourceScope::SharedPublic,
            canonical_key: "rss:official".to_owned(),
            name: "Gemini Blog".to_owned(),
            handle_or_url: "https://example.org/rss".to_owned(),
            category: "Official".to_owned(),
        };

        let rules = vec![
            WatchlistRuleInput {
                id: None,
                name: "Gemini Launch".to_owned(),
                keywords: vec!["Gemini".to_owned()],
                exclude_keywords: vec!["deprecated".to_owned()],
                priority: AlertPriority::Urgent,
                notify_immediately: true,
                enabled: true,
            },
            WatchlistRuleInput {
                id: None,
                name: "SDK".to_owned(),
                keywords: vec!["SDK".to_owned()],
                exclude_keywords: Vec::new(),
                priority: AlertPriority::High,
                notify_immediately: false,
                enabled: true,
            },
        ];

        let raw_items = vec![
            // 1. Excluded keyword suppresses "Gemini" rule
            RawCollectedItem {
                external_id: Some("1".to_owned()),
                url: "https://example.org/posts/old?utm_source=twitter".to_owned(),
                title: "Legacy Gemini deprecated notice".to_owned(),
                author: Some("Team".to_owned()),
                summary: None,
                published_at: Some(now - Duration::hours(72)),
            },
            // 2. High priority SDK item
            RawCollectedItem {
                external_id: Some("2".to_owned()),
                url: "https://example.org/posts/sdk?b=2&a=1&utm_medium=rss".to_owned(),
                title: "New SDK released".to_owned(),
                author: None,
                summary: Some("Typed Rust and TS SDK updates.".to_owned()),
                published_at: Some(now - Duration::hours(3)),
            },
            // 3. Duplicate canonical URL of item #2 (different tracking param)
            RawCollectedItem {
                external_id: Some("2-dup".to_owned()),
                url: "HTTPS://EXAMPLE.ORG:443/posts/sdk?a=1&b=2&fbclid=xyz#".to_owned(),
                title: "New SDK released".to_owned(),
                author: None,
                summary: Some("Typed Rust and TS SDK updates.".to_owned()),
                published_at: Some(now - Duration::hours(3)),
            },
            // 4. Urgent item matching Gemini
            RawCollectedItem {
                external_id: Some("3".to_owned()),
                url: "https://example.org/posts/gemini-2-5".to_owned(),
                title: "Gemini 2.5 GA".to_owned(),
                author: None,
                summary: Some("Generally available today.".to_owned()),
                published_at: Some(now - Duration::hours(1)),
            },
            // 5. Duplicate content hash of item #4 (different URL, identical normalized title/summary? Wait: hash includes URL, so same URL + different query order tests URL dedup; let's test same canonical URL + different title)
        ];

        let pipeline = ItemProcessingPipeline::new();
        let processed = pipeline
            .process_items(Some(&source), raw_items.clone(), &rules)
            .await
            .unwrap();

        assert_eq!(processed.len(), 3);
        // Urgent first
        assert_eq!(processed[0].title, "Gemini 2.5 GA");
        assert_eq!(processed[0].match_result.priority, AlertPriority::Urgent);
        assert!(processed[0].match_result.notify_immediately);
        assert_eq!(processed[0].source_id, Some(source.id));

        // High second, with normalized URL and sorted query params
        assert_eq!(processed[1].title, "New SDK released");
        assert_eq!(
            processed[1].canonical_url,
            "https://example.org/posts/sdk?a=1&b=2"
        );
        assert_eq!(processed[1].match_result.priority, AlertPriority::High);

        // Excluded third
        assert_eq!(processed[2].title, "Legacy Gemini deprecated notice");
        assert_eq!(processed[2].match_result.priority, AlertPriority::Normal);
        assert!(processed[2].match_result.matched_keywords.is_empty());

        // Also verify ItemProcessor compatibility trait
        let raw_roundtrip = pipeline.process(raw_items).await.unwrap();
        assert_eq!(raw_roundtrip.len(), 3);
    }
}
