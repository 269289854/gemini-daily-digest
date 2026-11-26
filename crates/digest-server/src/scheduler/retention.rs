//! Background 180-day retention pruner for unbookmarked canonical item bodies.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use digest_core::error::CoreError;

use crate::repo::Repositories;

/// Computes the retention cutoff timestamp and prunes unbookmarked `canonical_items.summary_raw`
/// older than `retention_days` (default `180`), setting `body_pruned = TRUE`.
pub async fn run_retention_once(
    repos: &Repositories,
    retention_days: u32,
    now: DateTime<Utc>,
) -> Result<u64, CoreError> {
    let cutoff = now - ChronoDuration::days(i64::from(retention_days));
    let pruned = repos.sources.prune_unbookmarked_bodies(cutoff).await?;
    if pruned > 0 {
        tracing::info!(
            pruned_items = pruned,
            retention_days,
            cutoff = %cutoff,
            "pruned unbookmarked canonical item bodies"
        );
    }
    Ok(pruned)
}

#[cfg(test)]
mod tests {
    use chrono::Duration as ChronoDuration;
    use digest_core::domain::{SourceScope, SourceType};
    use uuid::Uuid;

    use super::*;
    use crate::repo::{NewCanonicalItem, SubscribeSourceCmd};
    use crate::scheduler::test_support::MockRepos;

    #[tokio::test]
    async fn retention_prunes_old_unbookmarked_bodies_while_preserving_bookmarked_and_recent() {
        let mock = MockRepos::new();
        let repos = mock.repositories();
        let user_id = Uuid::new_v4();

        let src = repos
            .sources
            .create_or_subscribe(
                user_id,
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "retention-rss".to_owned(),
                    custom_name: "Test".to_owned(),
                    default_name: "Test".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: "https://example.com/feed.xml".to_owned(),
                    config_json_enc: None,
                    enabled: true,
                },
            )
            .await
            .unwrap();

        let now = Utc::now();
        let inserted = repos
            .sources
            .upsert_canonical_items(
                src.id,
                &[
                    NewCanonicalItem {
                        external_id: Some("old-unbookmarked".to_owned()),
                        canonical_url: "https://example.com/1".to_owned(),
                        content_hash: "h1".to_owned(),
                        title: "Old Unbookmarked".to_owned(),
                        author: None,
                        summary_raw: Some("Old body".to_owned()),
                        published_at: Some(now - ChronoDuration::days(200)),
                    },
                    NewCanonicalItem {
                        external_id: Some("old-bookmarked".to_owned()),
                        canonical_url: "https://example.com/2".to_owned(),
                        content_hash: "h2".to_owned(),
                        title: "Old Bookmarked".to_owned(),
                        author: None,
                        summary_raw: Some("Preserved body".to_owned()),
                        published_at: Some(now - ChronoDuration::days(200)),
                    },
                    NewCanonicalItem {
                        external_id: Some("recent".to_owned()),
                        canonical_url: "https://example.com/3".to_owned(),
                        content_hash: "h3".to_owned(),
                        title: "Recent".to_owned(),
                        author: None,
                        summary_raw: Some("Recent body".to_owned()),
                        published_at: Some(now - ChronoDuration::days(10)),
                    },
                ],
            )
            .await
            .unwrap();

        // Age the first two items in the mock store to > 180 days ago.
        mock.sources
            .set_item_fetched_at(inserted[0].id, now - ChronoDuration::days(190));
        mock.sources
            .set_item_fetched_at(inserted[1].id, now - ChronoDuration::days(190));

        // Bookmark item #2.
        repos
            .sources
            .upsert_bookmark(user_id, inserted[1].id, Some("Keep".to_owned()))
            .await
            .unwrap();

        let pruned = run_retention_once(&repos, 180, now).await.unwrap();
        assert_eq!(pruned, 1);

        let items = mock.sources.all_canonical_items();
        let old_unbookmarked = items.iter().find(|i| i.id == inserted[0].id).unwrap();
        let old_bookmarked = items.iter().find(|i| i.id == inserted[1].id).unwrap();
        let recent = items.iter().find(|i| i.id == inserted[2].id).unwrap();

        assert!(old_unbookmarked.body_pruned);
        assert!(old_unbookmarked.summary_raw.is_none());
        assert!(!old_bookmarked.body_pruned);
        assert_eq!(
            old_bookmarked.summary_raw.as_deref(),
            Some("Preserved body")
        );
        assert!(!recent.body_pruned);
        assert_eq!(recent.summary_raw.as_deref(), Some("Recent body"));
    }
}
