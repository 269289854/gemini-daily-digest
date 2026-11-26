//! Deterministic SHA-256 content hashing and batch deduplication.

use std::collections::HashSet;

use sha2::{Digest, Sha256};

use crate::domain::ProcessedItem;

/// Normalizes text for hashing by trimming, collapsing whitespace, and lowercasing.
#[must_use]
pub fn normalize_text_for_hash(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Computes a deterministic SHA-256 hex digest over canonical URL and normalized title/summary.
#[must_use]
pub fn compute_content_hash(canonical_url: &str, title: &str, summary: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical_url.as_bytes());
    hasher.update(b"\n");
    hasher.update(normalize_text_for_hash(title).as_bytes());
    hasher.update(b"\n");
    hasher.update(normalize_text_for_hash(summary.unwrap_or("")).as_bytes());
    hex::encode(hasher.finalize())
}

/// Deduplicates a batch of processed items by both `canonical_url` and `content_hash`.
#[must_use]
pub fn deduplicate_processed_items(items: Vec<ProcessedItem>) -> Vec<ProcessedItem> {
    let mut seen_urls = HashSet::with_capacity(items.len());
    let mut seen_hashes = HashSet::with_capacity(items.len());
    let mut unique = Vec::with_capacity(items.len());

    for item in items {
        let is_new_url = seen_urls.insert(item.canonical_url.clone());
        let is_new_hash = seen_hashes.insert(item.content_hash.clone());
        if is_new_url && is_new_hash {
            unique.push(item);
        }
    }

    unique
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ItemMatchResult;

    fn sample_item(url: &str, hash: &str) -> ProcessedItem {
        ProcessedItem {
            external_id: None,
            canonical_url: url.to_owned(),
            content_hash: hash.to_owned(),
            title: "Title".to_owned(),
            author: None,
            summary: None,
            published_at: None,
            source_id: None,
            source_name: None,
            category: None,
            match_result: ItemMatchResult::default(),
        }
    }

    #[test]
    fn content_hash_is_deterministic_across_whitespace_and_case() {
        let h1 = compute_content_hash(
            "https://example.org/a",
            "  Gemini   2.5 Pro ",
            Some("New   Release\nNotes"),
        );
        let h2 = compute_content_hash(
            "https://example.org/a",
            "gemini 2.5 pro",
            Some("new release notes"),
        );
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn deduplicate_removes_duplicate_urls_and_duplicate_hashes() {
        let items = vec![
            sample_item("https://example.org/1", "hash-a"),
            sample_item("https://example.org/1", "hash-b"),
            sample_item("https://example.org/2", "hash-a"),
            sample_item("https://example.org/3", "hash-c"),
        ];
        let deduped = deduplicate_processed_items(items);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].canonical_url, "https://example.org/1");
        assert_eq!(deduped[1].canonical_url, "https://example.org/3");
    }
}
