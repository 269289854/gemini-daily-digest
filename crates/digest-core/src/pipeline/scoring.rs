//! Deterministic importance scoring and item ordering.

use chrono::{DateTime, Duration, Utc};

pub use crate::constants::DEFAULT_SUMMARY_RICHNESS_BONUS;
use crate::constants::{
    DEFAULT_BASE_IMPORTANCE_SCORE, DEFAULT_HIGH_PRIORITY_BONUS, DEFAULT_KEYWORD_MATCH_BONUS,
    DEFAULT_RECENCY_24H_BONUS, DEFAULT_URGENT_PRIORITY_BONUS,
};
use crate::domain::{AlertPriority, ProcessedItem};
use crate::pipeline::watchlist::priority_rank;

/// Configurable scoring weights defaulting to centralized `constants`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoringWeights {
    pub base_score: i32,
    pub keyword_match_bonus: i32,
    pub urgent_priority_bonus: i32,
    pub high_priority_bonus: i32,
    pub recency_24h_bonus: i32,
    pub summary_richness_bonus: i32,
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            base_score: DEFAULT_BASE_IMPORTANCE_SCORE,
            keyword_match_bonus: DEFAULT_KEYWORD_MATCH_BONUS,
            urgent_priority_bonus: DEFAULT_URGENT_PRIORITY_BONUS,
            high_priority_bonus: DEFAULT_HIGH_PRIORITY_BONUS,
            recency_24h_bonus: DEFAULT_RECENCY_24H_BONUS,
            summary_richness_bonus: DEFAULT_SUMMARY_RICHNESS_BONUS,
        }
    }
}

/// Computes a deterministic importance score relative to `now`.
#[must_use]
pub fn score_item_at(
    published_at: Option<DateTime<Utc>>,
    summary: Option<&str>,
    matched_keywords: &[String],
    priority: AlertPriority,
    now: DateTime<Utc>,
    weights: &ScoringWeights,
) -> i32 {
    let mut score = weights.base_score;

    if let Some(ts) = published_at {
        if now.signed_duration_since(ts) <= Duration::hours(24) {
            score = score.saturating_add(weights.recency_24h_bonus);
        }
    }

    let keyword_count = i32::try_from(matched_keywords.len()).unwrap_or(i32::MAX);
    score = score.saturating_add(keyword_count.saturating_mul(weights.keyword_match_bonus));

    let priority_bonus = match priority {
        AlertPriority::Urgent => weights.urgent_priority_bonus,
        AlertPriority::High => weights.high_priority_bonus,
        AlertPriority::Normal => 0,
    };
    score = score.saturating_add(priority_bonus);

    if summary.is_some_and(|s| !s.trim().is_empty()) {
        score = score.saturating_add(weights.summary_richness_bonus);
    }

    score
}

/// Sorts items descending by `(priority_rank, importance_score, published_at)`
/// with a deterministic URL tie-breaker.
pub fn sort_processed_items(items: &mut [ProcessedItem]) {
    items.sort_by(|a, b| {
        priority_rank(b.match_result.priority)
            .cmp(&priority_rank(a.match_result.priority))
            .then_with(|| {
                b.match_result
                    .importance_score
                    .cmp(&a.match_result.importance_score)
            })
            .then_with(|| b.published_at.cmp(&a.published_at))
            .then_with(|| a.canonical_url.cmp(&b.canonical_url))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ItemMatchResult;

    #[test]
    fn score_item_and_sort_apply_weights_and_ordering() {
        let now = Utc::now();
        let weights = ScoringWeights::default();

        let score_urgent = score_item_at(
            Some(now - Duration::hours(2)),
            Some("Rich summary"),
            &["Gemini".to_owned()],
            AlertPriority::Urgent,
            now,
            &weights,
        );
        // base(10) + recency(15) + kw(25) + urgent(50) + summary(5) = 105
        assert_eq!(score_urgent, 105);

        let score_normal = score_item_at(
            Some(now - Duration::hours(48)),
            None,
            &[],
            AlertPriority::Normal,
            now,
            &weights,
        );
        assert_eq!(score_normal, DEFAULT_BASE_IMPORTANCE_SCORE);

        let mut items = vec![
            ProcessedItem {
                external_id: None,
                canonical_url: "https://example.org/low".to_owned(),
                content_hash: "h1".to_owned(),
                title: "Old".to_owned(),
                author: None,
                summary: None,
                published_at: Some(now - Duration::hours(48)),
                source_id: None,
                source_name: None,
                category: None,
                match_result: ItemMatchResult {
                    matched_keywords: Vec::new(),
                    priority: AlertPriority::Normal,
                    importance_score: score_normal,
                    notify_immediately: false,
                },
            },
            ProcessedItem {
                external_id: None,
                canonical_url: "https://example.org/high".to_owned(),
                content_hash: "h2".to_owned(),
                title: "Urgent".to_owned(),
                author: None,
                summary: Some("Rich summary".to_owned()),
                published_at: Some(now - Duration::hours(2)),
                source_id: None,
                source_name: None,
                category: None,
                match_result: ItemMatchResult {
                    matched_keywords: vec!["Gemini".to_owned()],
                    priority: AlertPriority::Urgent,
                    importance_score: score_urgent,
                    notify_immediately: true,
                },
            },
        ];

        sort_processed_items(&mut items);
        assert_eq!(items[0].canonical_url, "https://example.org/high");
        assert_eq!(items[1].canonical_url, "https://example.org/low");
    }
}
