//! Two-layer decoupled scheduler, high-frequency alert scanner, retention worker,
//! and `ServerExecutionOrchestrator`.

pub mod alert_scanner;
pub mod composer;
pub mod executor;
pub mod poller;
pub mod retention;

use std::time::Duration;

use chrono::Utc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::constants::{
    SCHEDULER_ALERT_TICK_SECS, SCHEDULER_COMPOSE_TICK_SECS, SCHEDULER_POLL_TICK_SECS,
    SCHEDULER_RETENTION_TICK_SECS,
};
pub use composer::ComposerDeps;
pub use executor::ServerExecutionOrchestrator;

/// Join handles for background scheduler tasks spawned by `spawn_scheduler_tasks`.
pub struct SchedulerHandles {
    tasks: Vec<JoinHandle<()>>,
}

impl SchedulerHandles {
    /// Awaits all background scheduler tasks until completion (typically after signaling shutdown).
    pub async fn join_all(self) {
        for handle in self.tasks {
            if let Err(err) = handle.await {
                tracing::warn!(error = %err, "scheduler background task terminated with join error");
            }
        }
    }
}

/// Spawns the 4 decoupled scheduler background loops (`poller`, `composer`, `alert_scanner`,
/// and `retention`) controlled by `shutdown_rx`.
#[must_use]
pub fn spawn_scheduler_tasks(
    deps: ComposerDeps,
    shutdown_rx: watch::Receiver<bool>,
) -> SchedulerHandles {
    let poller_handle = spawn_poller_loop(deps.clone(), shutdown_rx.clone());
    let composer_handle = spawn_composer_loop(deps.clone(), shutdown_rx.clone());
    let alert_handle = spawn_alert_loop(deps.clone(), shutdown_rx.clone());
    let retention_handle = spawn_retention_loop(deps, shutdown_rx);

    SchedulerHandles {
        tasks: vec![
            poller_handle,
            composer_handle,
            alert_handle,
            retention_handle,
        ],
    }
}

fn spawn_poller_loop(deps: ComposerDeps, mut shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(SCHEDULER_POLL_TICK_SECS));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    if let Err(err) = poller::run_poller_tick(
                        &deps.repos,
                        &deps.cache,
                        &deps.cipher,
                        &deps.collectors,
                        &deps.pipeline,
                        &deps.config.core.collector,
                        &deps.config.core.resources,
                    ).await {
                        tracing::error!(error = %err, "Layer 1 poller tick failed");
                    }
                }
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    })
}

fn spawn_composer_loop(
    deps: ComposerDeps,
    mut shutdown_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(SCHEDULER_COMPOSE_TICK_SECS));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    if let Err(err) = composer::run_composer_tick(&deps).await {
                        tracing::error!(error = %err, "Layer 2 composer tick failed");
                    }
                }
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    })
}

fn spawn_alert_loop(deps: ComposerDeps, mut shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(SCHEDULER_ALERT_TICK_SECS));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    if let Err(err) = alert_scanner::run_alert_scan_tick(&deps).await {
                        tracing::error!(error = %err, "alert scanner tick failed");
                    }
                }
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    })
}

fn spawn_retention_loop(
    deps: ComposerDeps,
    mut shutdown_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(SCHEDULER_RETENTION_TICK_SECS));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    if let Err(err) = retention::run_retention_once(
                        &deps.repos,
                        deps.config.core.resources.item_body_retention_days,
                        Utc::now(),
                    ).await {
                        tracing::error!(error = %err, "retention pruner tick failed");
                    }
                }
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
    use digest_core::constants::{
        DEFAULT_ALERT_INTERVAL_MINUTES, DEFAULT_DAILY_TIME, DEFAULT_TIMEZONE,
    };
    use digest_core::crypto::EnvelopeCipher;
    use digest_core::domain::{AlertPriority, DeliveryStatus, RunStatus, SourceScope};
    use digest_core::error::CoreError;
    use uuid::Uuid;

    use crate::repo::*;

    #[derive(Clone)]
    pub(crate) struct MockRepos {
        pub users: Arc<MockUserRepo>,
        pub sources: Arc<MockSourceRepo>,
        pub digests: Arc<MockDigestRepo>,
        pub runs: Arc<MockRunRepo>,
        pub settings: Arc<MockSettingsRepo>,
    }

    impl MockRepos {
        pub fn new() -> Self {
            Self {
                users: Arc::new(MockUserRepo::default()),
                sources: Arc::new(MockSourceRepo::default()),
                digests: Arc::new(MockDigestRepo::default()),
                runs: Arc::new(MockRunRepo::default()),
                settings: Arc::new(MockSettingsRepo::default()),
            }
        }

        pub fn repositories(&self) -> Repositories {
            Repositories {
                users: Arc::clone(&self.users) as Arc<dyn UserRepo>,
                sources: Arc::clone(&self.sources) as Arc<dyn SourceRepo>,
                digests: Arc::clone(&self.digests) as Arc<dyn DigestRepo>,
                runs: Arc::clone(&self.runs) as Arc<dyn RunRepo>,
                settings: Arc::clone(&self.settings) as Arc<dyn SettingsRepo>,
            }
        }
    }

    #[derive(Default)]
    pub(crate) struct MockUserRepo {
        users: Mutex<HashMap<Uuid, UserRecord>>,
    }

    #[async_trait]
    impl UserRepo for MockUserRepo {
        async fn upsert_oidc_user(&self, cmd: UpsertUserCmd) -> Result<UserRecord, CoreError> {
            let mut guard = self.users.lock().unwrap();
            if let Some(existing) = guard.values_mut().find(|u| u.oidc_sub == cmd.oidc_sub) {
                existing.email = cmd.email;
                existing.display_name = cmd.display_name;
                existing.last_login_at = Some(Utc::now());
                return Ok(existing.clone());
            }
            let rec = UserRecord {
                id: Uuid::new_v4(),
                oidc_sub: cmd.oidc_sub,
                email: cmd.email,
                display_name: cmd.display_name,
                role: cmd.role,
                created_at: Utc::now(),
                last_login_at: Some(Utc::now()),
            };
            guard.insert(rec.id, rec.clone());
            Ok(rec)
        }

        async fn get_user_by_id(&self, user_id: Uuid) -> Result<Option<UserRecord>, CoreError> {
            Ok(self.users.lock().unwrap().get(&user_id).cloned())
        }

        async fn get_onboarding_flags(&self, _user_id: Uuid) -> Result<OnboardingFlags, CoreError> {
            Ok(OnboardingFlags::default())
        }
    }

    #[derive(Default)]
    struct SourceStore {
        canonical_sources: HashMap<Uuid, CanonicalSourceRecord>,
        subscriptions: Vec<SubscriptionEntry>,
        items: Vec<CanonicalItemRecord>,
        matches: HashMap<(Uuid, Uuid), UserItemMatchRow>,
        bookmarks: HashMap<(Uuid, Uuid), BookmarkView>,
    }

    #[derive(Clone)]
    struct SubscriptionEntry {
        id: Uuid,
        user_id: Uuid,
        canonical_source_id: Uuid,
        custom_name: String,
        category: String,
        enabled: bool,
        created_at: DateTime<Utc>,
    }

    #[derive(Clone)]
    struct UserItemMatchRow {
        user_id: Uuid,
        item_id: Uuid,
        matched_keywords: Vec<String>,
        priority: AlertPriority,
        importance_score: i32,
        alerted_at: Option<DateTime<Utc>>,
    }

    #[derive(Default)]
    pub(crate) struct MockSourceRepo {
        store: Mutex<SourceStore>,
    }

    impl MockSourceRepo {
        pub fn all_canonical_sources(&self) -> Vec<CanonicalSourceRecord> {
            self.store
                .lock()
                .unwrap()
                .canonical_sources
                .values()
                .cloned()
                .collect()
        }

        pub fn all_canonical_items(&self) -> Vec<CanonicalItemRecord> {
            self.store.lock().unwrap().items.clone()
        }

        pub fn set_item_fetched_at(&self, item_id: Uuid, fetched_at: DateTime<Utc>) {
            let mut guard = self.store.lock().unwrap();
            if let Some(item) = guard.items.iter_mut().find(|i| i.id == item_id) {
                item.fetched_at = fetched_at;
            }
        }
    }

    #[async_trait]
    impl SourceRepo for MockSourceRepo {
        async fn create_or_subscribe(
            &self,
            user_id: Uuid,
            cmd: SubscribeSourceCmd,
        ) -> Result<UserSourceView, CoreError> {
            let mut guard = self.store.lock().unwrap();
            let owner_id = cmd.scope_owner_id(user_id);
            let canonical_id = if let Some(existing) = guard.canonical_sources.values().find(|s| {
                s.source_type == cmd.source_type
                    && s.canonical_key == cmd.canonical_key
                    && s.scope_owner_id == owner_id
            }) {
                existing.id
            } else {
                let id = Uuid::new_v4();
                guard.canonical_sources.insert(
                    id,
                    CanonicalSourceRecord {
                        id,
                        source_type: cmd.source_type,
                        canonical_key: cmd.canonical_key.clone(),
                        scope: cmd.scope,
                        scope_owner_id: owner_id,
                        default_name: cmd.default_name.clone(),
                        config_json_enc: cmd.config_json_enc.clone(),
                        etag: None,
                        last_modified: None,
                        last_fetched_at: None,
                        next_poll_at: None,
                        last_status: None,
                        last_error: None,
                        consecutive_failures: 0,
                        created_at: Utc::now(),
                    },
                );
                id
            };

            let sub_id = Uuid::new_v4();
            guard.subscriptions.push(SubscriptionEntry {
                id: sub_id,
                user_id,
                canonical_source_id: canonical_id,
                custom_name: cmd.custom_name.clone(),
                category: cmd.category.clone(),
                enabled: cmd.enabled,
                created_at: Utc::now(),
            });

            let src = guard.canonical_sources.get(&canonical_id).unwrap();
            Ok(UserSourceView {
                id: canonical_id,
                subscription_id: sub_id,
                source_type: src.source_type,
                scope: src.scope,
                canonical_key: src.canonical_key.clone(),
                custom_name: cmd.custom_name,
                default_name: src.default_name.clone(),
                category: cmd.category,
                handle_or_url: cmd.handle_or_url,
                enabled: cmd.enabled,
                has_credentials: false,
                config_hint: None,
                last_fetched_at: src.last_fetched_at,
                next_poll_at: src.next_poll_at,
                last_status: src.last_status,
                last_error: src.last_error.clone(),
                consecutive_failures: src.consecutive_failures,
                created_at: src.created_at,
            })
        }

        async fn list_user_sources(&self, user_id: Uuid) -> Result<Vec<UserSourceView>, CoreError> {
            let guard = self.store.lock().unwrap();
            let dummy_cipher = EnvelopeCipher::from_bytes([0u8; 32]);
            let mut out = Vec::new();
            for sub in guard.subscriptions.iter().filter(|s| s.user_id == user_id) {
                if let Some(src) = guard.canonical_sources.get(&sub.canonical_source_id) {
                    let rec = UserSourceRecord {
                        subscription_id: sub.id,
                        canonical_source_id: src.id,
                        user_id,
                        source_type: src.source_type,
                        scope: src.scope,
                        scope_owner_id: src.scope_owner_id,
                        canonical_key: src.canonical_key.clone(),
                        default_name: src.default_name.clone(),
                        custom_name: sub.custom_name.clone(),
                        category: sub.category.clone(),
                        enabled: sub.enabled,
                        config_json_enc: None,
                        etag: src.etag.clone(),
                        last_modified: src.last_modified.clone(),
                        last_fetched_at: src.last_fetched_at,
                        next_poll_at: src.next_poll_at,
                        last_status: src.last_status,
                        last_error: src.last_error.clone(),
                        consecutive_failures: src.consecutive_failures,
                        created_at: sub.created_at,
                    };
                    out.push(
                        rec.to_view(&dummy_cipher)
                            .unwrap_or_else(|_| UserSourceView {
                                id: src.id,
                                subscription_id: sub.id,
                                source_type: src.source_type,
                                scope: src.scope,
                                canonical_key: src.canonical_key.clone(),
                                custom_name: sub.custom_name.clone(),
                                default_name: src.default_name.clone(),
                                category: sub.category.clone(),
                                handle_or_url: src.canonical_key.clone(),
                                enabled: sub.enabled,
                                has_credentials: false,
                                config_hint: None,
                                last_fetched_at: src.last_fetched_at,
                                next_poll_at: src.next_poll_at,
                                last_status: src.last_status,
                                last_error: src.last_error.clone(),
                                consecutive_failures: src.consecutive_failures,
                                created_at: sub.created_at,
                            }),
                    );
                }
            }
            Ok(out)
        }

        async fn get_user_source(
            &self,
            user_id: Uuid,
            id_or_sub: Uuid,
        ) -> Result<Option<UserSourceRecord>, CoreError> {
            let guard = self.store.lock().unwrap();
            let Some(sub) = guard.subscriptions.iter().find(|s| {
                s.user_id == user_id && (s.canonical_source_id == id_or_sub || s.id == id_or_sub)
            }) else {
                return Ok(None);
            };
            let Some(src) = guard.canonical_sources.get(&sub.canonical_source_id) else {
                return Ok(None);
            };
            Ok(Some(UserSourceRecord {
                subscription_id: sub.id,
                canonical_source_id: src.id,
                user_id,
                source_type: src.source_type,
                scope: src.scope,
                scope_owner_id: src.scope_owner_id,
                canonical_key: src.canonical_key.clone(),
                default_name: src.default_name.clone(),
                custom_name: sub.custom_name.clone(),
                category: sub.category.clone(),
                enabled: sub.enabled,
                config_json_enc: src.config_json_enc.clone(),
                etag: src.etag.clone(),
                last_modified: src.last_modified.clone(),
                last_fetched_at: src.last_fetched_at,
                next_poll_at: src.next_poll_at,
                last_status: src.last_status,
                last_error: src.last_error.clone(),
                consecutive_failures: src.consecutive_failures,
                created_at: sub.created_at,
            }))
        }

        async fn update_user_subscription(
            &self,
            _user_id: Uuid,
            _id_or_sub: Uuid,
            _cmd: UpdateSourceCmd,
        ) -> Result<UserSourceView, CoreError> {
            Err(CoreError::NotFound("not implemented in mock".to_owned()))
        }

        async fn delete_user_subscription(
            &self,
            _user_id: Uuid,
            _id_or_sub: Uuid,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn list_pollable_sources(
            &self,
            scope: Option<SourceScope>,
            due_before: DateTime<Utc>,
            limit: usize,
        ) -> Result<Vec<CanonicalSourceRecord>, CoreError> {
            let guard = self.store.lock().unwrap();
            let enabled_source_ids: HashSet<Uuid> = guard
                .subscriptions
                .iter()
                .filter(|s| s.enabled)
                .map(|s| s.canonical_source_id)
                .collect();

            let mut out: Vec<CanonicalSourceRecord> = guard
                .canonical_sources
                .values()
                .filter(|s| enabled_source_ids.contains(&s.id))
                .filter(|s| scope.is_none_or(|sc| s.scope == sc))
                .filter(|s| s.next_poll_at.is_none_or(|due| due <= due_before))
                .cloned()
                .collect();
            out.truncate(limit);
            Ok(out)
        }

        async fn record_fetch_outcome(
            &self,
            source_id: Uuid,
            update: SourceFetchStateUpdate,
        ) -> Result<(), CoreError> {
            let mut guard = self.store.lock().unwrap();
            if let Some(src) = guard.canonical_sources.get_mut(&source_id) {
                src.last_status = Some(update.outcome);
                src.etag = update.etag;
                src.last_modified = update.last_modified;
                src.last_fetched_at = Some(update.fetched_at);
                src.next_poll_at = update.next_poll_at;
                src.last_error = update.last_error;
                src.consecutive_failures = update.consecutive_failures;
            }
            Ok(())
        }

        async fn upsert_canonical_items(
            &self,
            source_id: Uuid,
            items: &[NewCanonicalItem],
        ) -> Result<Vec<CanonicalItemRecord>, CoreError> {
            let mut guard = self.store.lock().unwrap();
            let mut inserted = Vec::new();
            for item in items {
                let conflict = guard.items.iter().any(|existing| {
                    existing.canonical_source_id == source_id
                        && (existing.canonical_url == item.canonical_url
                            || existing.content_hash == item.content_hash)
                });
                if conflict {
                    continue;
                }
                let rec = CanonicalItemRecord {
                    id: Uuid::new_v4(),
                    canonical_source_id: source_id,
                    external_id: item.external_id.clone(),
                    canonical_url: item.canonical_url.clone(),
                    content_hash: item.content_hash.clone(),
                    title: item.title.clone(),
                    author: item.author.clone(),
                    summary_raw: item.summary_raw.clone(),
                    body_pruned: false,
                    published_at: item.published_at,
                    fetched_at: Utc::now(),
                };
                guard.items.push(rec.clone());
                inserted.push(rec);
            }
            Ok(inserted)
        }

        async fn list_recent_source_items(
            &self,
            source_id: Uuid,
            since: DateTime<Utc>,
            limit: usize,
        ) -> Result<Vec<CanonicalItemRecord>, CoreError> {
            let guard = self.store.lock().unwrap();
            let mut items: Vec<CanonicalItemRecord> = guard
                .items
                .iter()
                .filter(|i| i.canonical_source_id == source_id && i.fetched_at >= since)
                .cloned()
                .collect();
            items.truncate(limit);
            Ok(items)
        }

        async fn list_source_items(
            &self,
            _user_id: Uuid,
            _source_id: Uuid,
            page: PageRequest,
        ) -> Result<PagedResult<ItemView>, CoreError> {
            Ok(PagedResult::new(Vec::new(), page, 0))
        }

        async fn search_user_items(
            &self,
            _user_id: Uuid,
            query: ItemSearchQuery,
        ) -> Result<PagedResult<ItemView>, CoreError> {
            Ok(PagedResult::new(Vec::new(), query.page, 0))
        }

        async fn upsert_user_item_matches(
            &self,
            user_id: Uuid,
            matches: &[NewUserItemMatch],
        ) -> Result<(), CoreError> {
            let mut guard = self.store.lock().unwrap();
            for m in matches {
                guard
                    .matches
                    .entry((user_id, m.item_id))
                    .and_modify(|existing| {
                        existing.matched_keywords = m.matched_keywords.clone();
                        existing.priority = m.priority;
                        existing.importance_score = m.importance_score;
                    })
                    .or_insert(UserItemMatchRow {
                        user_id,
                        item_id: m.item_id,
                        matched_keywords: m.matched_keywords.clone(),
                        priority: m.priority,
                        importance_score: m.importance_score,
                        alerted_at: None,
                    });
            }
            Ok(())
        }

        async fn list_pending_urgent_alerts(
            &self,
            user_id: Uuid,
        ) -> Result<Vec<PendingAlertItem>, CoreError> {
            let guard = self.store.lock().unwrap();
            let mut out = Vec::new();
            for row in guard.matches.values().filter(|m| {
                m.user_id == user_id
                    && m.alerted_at.is_none()
                    && m.priority == AlertPriority::Urgent
            }) {
                if let Some(item) = guard.items.iter().find(|i| i.id == row.item_id) {
                    let sub = guard.subscriptions.iter().find(|s| {
                        s.user_id == user_id && s.canonical_source_id == item.canonical_source_id
                    });
                    out.push(PendingAlertItem {
                        item_id: item.id,
                        source_id: item.canonical_source_id,
                        source_name: sub.map(|s| s.custom_name.clone()).unwrap_or_default(),
                        category: sub.map(|s| s.category.clone()).unwrap_or_default(),
                        canonical_url: item.canonical_url.clone(),
                        title: item.title.clone(),
                        author: item.author.clone(),
                        summary: item.summary_raw.clone(),
                        published_at: item.published_at,
                        matched_keywords: row.matched_keywords.clone(),
                        priority: row.priority,
                        importance_score: row.importance_score,
                    });
                }
            }
            Ok(out)
        }

        async fn mark_items_alerted(
            &self,
            user_id: Uuid,
            item_ids: &[Uuid],
            alerted_at: DateTime<Utc>,
        ) -> Result<(), CoreError> {
            let mut guard = self.store.lock().unwrap();
            for item_id in item_ids {
                if let Some(m) = guard.matches.get_mut(&(user_id, *item_id)) {
                    m.alerted_at = Some(alerted_at);
                }
            }
            Ok(())
        }

        async fn prune_unbookmarked_bodies(&self, cutoff: DateTime<Utc>) -> Result<u64, CoreError> {
            let mut guard = self.store.lock().unwrap();
            let bookmarked: HashSet<Uuid> = guard
                .bookmarks
                .keys()
                .map(|(_, item_id)| *item_id)
                .collect();
            let mut count = 0u64;
            for item in &mut guard.items {
                if !item.body_pruned && item.fetched_at < cutoff && !bookmarked.contains(&item.id) {
                    item.summary_raw = None;
                    item.body_pruned = true;
                    count += 1;
                }
            }
            Ok(count)
        }

        async fn upsert_bookmark(
            &self,
            user_id: Uuid,
            item_id: Uuid,
            note: Option<String>,
        ) -> Result<BookmarkView, CoreError> {
            let view = BookmarkView {
                id: Uuid::new_v4(),
                item_id,
                note,
                created_at: Utc::now(),
            };
            self.store
                .lock()
                .unwrap()
                .bookmarks
                .insert((user_id, item_id), view.clone());
            Ok(view)
        }

        async fn delete_bookmark(&self, user_id: Uuid, item_id: Uuid) -> Result<(), CoreError> {
            self.store
                .lock()
                .unwrap()
                .bookmarks
                .remove(&(user_id, item_id));
            Ok(())
        }
    }

    #[derive(Default)]
    pub(crate) struct MockDigestRepo {
        digests: Mutex<HashMap<(Uuid, NaiveDate), DailyDigestDetail>>,
        source_digests: Mutex<Vec<SourceDailyDigestView>>,
    }

    #[async_trait]
    impl DigestRepo for MockDigestRepo {
        async fn list_daily_digests(
            &self,
            user_id: Uuid,
            page: PageRequest,
        ) -> Result<PagedResult<DailyDigestSummary>, CoreError> {
            let guard = self.digests.lock().unwrap();
            let items: Vec<DailyDigestSummary> = guard
                .iter()
                .filter(|((uid, _), _)| *uid == user_id)
                .map(|(_, d)| DailyDigestSummary {
                    id: d.id,
                    digest_date: d.digest_date,
                    title: d.title.clone(),
                    item_count: d.item_count,
                    highlighted_count: d.highlighted_count,
                    prompt_tokens: d.prompt_tokens,
                    completion_tokens: d.completion_tokens,
                    model_used: d.model_used.clone(),
                    degraded_fallback: d.degraded_fallback,
                    created_at: d.created_at,
                })
                .collect();
            let total = items.len() as u64;
            Ok(PagedResult::new(items, page, total))
        }

        async fn get_daily_digest_by_date(
            &self,
            user_id: Uuid,
            date: NaiveDate,
        ) -> Result<Option<DailyDigestDetail>, CoreError> {
            Ok(self.digests.lock().unwrap().get(&(user_id, date)).cloned())
        }

        async fn find_daily_digest_by_fingerprint(
            &self,
            user_id: Uuid,
            date: NaiveDate,
            fingerprint: &str,
        ) -> Result<Option<DailyDigestDetail>, CoreError> {
            Ok(self
                .digests
                .lock()
                .unwrap()
                .get(&(user_id, date))
                .filter(|d| d.input_fingerprint == fingerprint)
                .cloned())
        }

        async fn save_daily_and_source_digests(
            &self,
            cmd: SaveDigestCmd,
        ) -> Result<DailyDigestDetail, CoreError> {
            let detail = DailyDigestDetail {
                id: Uuid::new_v4(),
                digest_date: cmd.digest_date,
                input_fingerprint: cmd.input_fingerprint,
                title: cmd.title,
                overview_markdown: cmd.overview_markdown,
                overview_html: cmd.overview_html,
                sections: cmd.sections,
                item_count: cmd.item_count,
                highlighted_count: cmd.highlighted_count,
                prompt_tokens: cmd.prompt_tokens,
                completion_tokens: cmd.completion_tokens,
                model_used: cmd.model_used,
                degraded_fallback: cmd.degraded_fallback,
                created_at: Utc::now(),
            };
            self.digests
                .lock()
                .unwrap()
                .insert((cmd.user_id, cmd.digest_date), detail.clone());

            let mut src_guard = self.source_digests.lock().unwrap();
            for s in cmd.source_digests {
                src_guard.push(SourceDailyDigestView {
                    id: Uuid::new_v4(),
                    digest_id: Some(detail.id),
                    source_id: s.canonical_source_id,
                    source_name: String::new(),
                    digest_date: s.digest_date,
                    input_fingerprint: s.input_fingerprint,
                    summary_markdown: s.summary_markdown,
                    key_points: s.key_points,
                    item_count: s.item_count,
                    created_at: Utc::now(),
                });
            }
            Ok(detail)
        }

        async fn list_source_daily_digests(
            &self,
            _user_id: Uuid,
            source_id: Uuid,
            page: PageRequest,
        ) -> Result<PagedResult<SourceDailyDigestView>, CoreError> {
            let items: Vec<SourceDailyDigestView> = self
                .source_digests
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.source_id == source_id)
                .cloned()
                .collect();
            let total = items.len() as u64;
            Ok(PagedResult::new(items, page, total))
        }
    }

    #[derive(Default)]
    pub(crate) struct MockRunRepo {
        runs: Mutex<Vec<DigestRunRecord>>,
        deliveries: Mutex<Vec<DeliveryLogRecord>>,
    }

    #[async_trait]
    impl RunRepo for MockRunRepo {
        async fn create_run(&self, cmd: CreateRunCmd) -> Result<DigestRunRecord, CoreError> {
            let rec = DigestRunRecord {
                id: Uuid::new_v4(),
                user_id: cmd.user_id,
                run_type: cmd.run_type,
                status: RunStatus::Running,
                started_at: cmd.started_at,
                finished_at: None,
                items_fetched: 0,
                items_new: 0,
                prompt_tokens: 0,
                completion_tokens: 0,
                error_summary: None,
            };
            self.runs.lock().unwrap().push(rec.clone());
            Ok(rec)
        }

        async fn finish_run(&self, run_id: Uuid, update: FinishRunCmd) -> Result<(), CoreError> {
            let mut guard = self.runs.lock().unwrap();
            if let Some(r) = guard.iter_mut().find(|r| r.id == run_id) {
                r.status = update.status;
                r.finished_at = Some(update.finished_at);
                r.items_fetched = update.items_fetched;
                r.items_new = update.items_new;
                r.prompt_tokens = update.prompt_tokens;
                r.completion_tokens = update.completion_tokens;
                r.error_summary = update.error_summary;
            }
            Ok(())
        }

        async fn list_runs_with_deliveries(
            &self,
            user_id: Uuid,
            page: PageRequest,
        ) -> Result<PagedResult<DigestRunView>, CoreError> {
            let runs = self.runs.lock().unwrap();
            let dels = self.deliveries.lock().unwrap();
            let items: Vec<DigestRunView> = runs
                .iter()
                .filter(|r| r.user_id == user_id)
                .map(|r| DigestRunView {
                    id: r.id,
                    run_type: r.run_type,
                    status: r.status,
                    started_at: r.started_at,
                    finished_at: r.finished_at,
                    items_fetched: r.items_fetched,
                    items_new: r.items_new,
                    prompt_tokens: r.prompt_tokens,
                    completion_tokens: r.completion_tokens,
                    error_summary: r.error_summary.clone(),
                    deliveries: dels.iter().filter(|d| d.run_id == r.id).cloned().collect(),
                })
                .collect();
            let total = items.len() as u64;
            Ok(PagedResult::new(items, page, total))
        }

        async fn record_delivery(
            &self,
            cmd: CreateDeliveryLogCmd,
        ) -> Result<DeliveryLogRecord, CoreError> {
            let rec = DeliveryLogRecord {
                id: Uuid::new_v4(),
                run_id: cmd.run_id,
                user_id: cmd.user_id,
                digest_id: cmd.digest_id,
                channel_id: cmd.channel_id,
                channel_type: cmd.channel_type,
                status: cmd.status,
                error_message: cmd.error_message,
                sent_at: cmd.sent_at,
            };
            self.deliveries.lock().unwrap().push(rec.clone());
            Ok(rec)
        }

        async fn get_delivery_by_id(
            &self,
            user_id: Uuid,
            delivery_id: Uuid,
        ) -> Result<Option<DeliveryLogRecord>, CoreError> {
            Ok(self
                .deliveries
                .lock()
                .unwrap()
                .iter()
                .find(|d| d.user_id == user_id && d.id == delivery_id)
                .cloned())
        }

        async fn update_delivery_status(
            &self,
            user_id: Uuid,
            delivery_id: Uuid,
            status: DeliveryStatus,
            error_message: Option<String>,
            sent_at: DateTime<Utc>,
        ) -> Result<DeliveryLogRecord, CoreError> {
            let mut guard = self.deliveries.lock().unwrap();
            let rec = guard
                .iter_mut()
                .find(|d| d.user_id == user_id && d.id == delivery_id)
                .ok_or_else(|| CoreError::NotFound("delivery not found".to_owned()))?;
            rec.status = status;
            rec.error_message = error_message;
            rec.sent_at = sent_at;
            Ok(rec.clone())
        }

        async fn get_run_stats(&self, _user_id: Uuid) -> Result<RunStatsView, CoreError> {
            Ok(RunStatsView {
                total_runs: 0,
                succeeded_runs: 0,
                failed_runs: 0,
                total_prompt_tokens: 0,
                total_completion_tokens: 0,
                deliveries_sent: 0,
                deliveries_failed: 0,
                source_health: Vec::new(),
            })
        }
    }

    #[derive(Default)]
    pub(crate) struct MockSettingsRepo {
        ai: Mutex<HashMap<Uuid, UserAiConfigRecord>>,
        channels: Mutex<Vec<NotificationChannelRecord>>,
        schedules: Mutex<HashMap<Uuid, UserScheduleRecord>>,
        watchlist: Mutex<Vec<WatchlistRuleRecord>>,
    }

    #[async_trait]
    impl SettingsRepo for MockSettingsRepo {
        async fn get_ai_config(
            &self,
            user_id: Uuid,
        ) -> Result<Option<UserAiConfigRecord>, CoreError> {
            Ok(self.ai.lock().unwrap().get(&user_id).cloned())
        }

        async fn upsert_ai_config(
            &self,
            user_id: Uuid,
            cmd: UpsertAiConfigRecord,
        ) -> Result<UserAiConfigRecord, CoreError> {
            let rec = UserAiConfigRecord {
                user_id,
                provider_label: cmd.provider_label,
                base_url: cmd.base_url,
                api_key_enc: cmd.api_key_enc,
                api_key_hint: cmd.api_key_hint,
                model: cmd.model,
                system_prompt: cmd.system_prompt,
                temperature: cmd.temperature,
                max_tokens: cmd.max_tokens,
                updated_at: Utc::now(),
            };
            self.ai.lock().unwrap().insert(user_id, rec.clone());
            Ok(rec)
        }

        async fn list_channels(
            &self,
            user_id: Uuid,
        ) -> Result<Vec<NotificationChannelRecord>, CoreError> {
            Ok(self
                .channels
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.user_id == user_id)
                .cloned()
                .collect())
        }

        async fn get_channel(
            &self,
            user_id: Uuid,
            channel_id: Uuid,
        ) -> Result<Option<NotificationChannelRecord>, CoreError> {
            Ok(self
                .channels
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.user_id == user_id && c.id == channel_id)
                .cloned())
        }

        async fn create_channel(
            &self,
            user_id: Uuid,
            cmd: CreateChannelRecord,
        ) -> Result<NotificationChannelRecord, CoreError> {
            let rec = NotificationChannelRecord {
                id: Uuid::new_v4(),
                user_id,
                channel_type: cmd.channel_type,
                name: cmd.name,
                enabled: cmd.enabled,
                config_enc: cmd.config_enc,
                config_hint: cmd.config_hint,
                trigger_on_daily: cmd.trigger_on_daily,
                trigger_on_alert: cmd.trigger_on_alert,
                created_at: Utc::now(),
            };
            self.channels.lock().unwrap().push(rec.clone());
            Ok(rec)
        }

        async fn update_channel(
            &self,
            _user_id: Uuid,
            _channel_id: Uuid,
            _cmd: UpdateChannelRecord,
        ) -> Result<NotificationChannelRecord, CoreError> {
            Err(CoreError::NotFound("not implemented".to_owned()))
        }

        async fn delete_channel(&self, _user_id: Uuid, _channel_id: Uuid) -> Result<(), CoreError> {
            Ok(())
        }

        async fn get_schedule(&self, user_id: Uuid) -> Result<UserScheduleRecord, CoreError> {
            let mut guard = self.schedules.lock().unwrap();
            let rec = guard.entry(user_id).or_insert_with(|| UserScheduleRecord {
                user_id,
                timezone: DEFAULT_TIMEZONE.to_owned(),
                daily_time: DEFAULT_DAILY_TIME.to_owned(),
                email_enabled: true,
                send_when_empty: false,
                alert_scan_enabled: true,
                alert_interval_minutes: i32::try_from(DEFAULT_ALERT_INTERVAL_MINUTES).unwrap_or(60),
                last_daily_run_date: None,
                last_alert_scan_at: None,
            });
            Ok(rec.clone())
        }

        async fn upsert_schedule(
            &self,
            user_id: Uuid,
            cmd: UpdateScheduleCmd,
        ) -> Result<UserScheduleRecord, CoreError> {
            let mut guard = self.schedules.lock().unwrap();
            let prev = guard.get(&user_id).cloned();
            let rec = UserScheduleRecord {
                user_id,
                timezone: cmd.timezone,
                daily_time: cmd.daily_time,
                email_enabled: cmd.email_enabled,
                send_when_empty: cmd.send_when_empty,
                alert_scan_enabled: cmd.alert_scan_enabled,
                alert_interval_minutes: cmd.alert_interval_minutes,
                last_daily_run_date: prev.as_ref().and_then(|p| p.last_daily_run_date),
                last_alert_scan_at: prev.and_then(|p| p.last_alert_scan_at),
            };
            guard.insert(user_id, rec.clone());
            Ok(rec)
        }

        async fn list_due_daily_schedules(&self) -> Result<Vec<UserScheduleRecord>, CoreError> {
            Ok(self.schedules.lock().unwrap().values().cloned().collect())
        }

        async fn mark_daily_run_date(
            &self,
            user_id: Uuid,
            digest_date: NaiveDate,
        ) -> Result<(), CoreError> {
            if let Some(s) = self.schedules.lock().unwrap().get_mut(&user_id) {
                s.last_daily_run_date = Some(digest_date);
            }
            Ok(())
        }

        async fn list_due_alert_schedules(
            &self,
            now: DateTime<Utc>,
        ) -> Result<Vec<UserScheduleRecord>, CoreError> {
            Ok(self
                .schedules
                .lock()
                .unwrap()
                .values()
                .filter(|s| {
                    s.alert_scan_enabled
                        && s.last_alert_scan_at.is_none_or(|last| {
                            last + ChronoDuration::minutes(i64::from(s.alert_interval_minutes))
                                <= now
                        })
                })
                .cloned()
                .collect())
        }

        async fn mark_alert_scan_at(
            &self,
            user_id: Uuid,
            scanned_at: DateTime<Utc>,
        ) -> Result<(), CoreError> {
            if let Some(s) = self.schedules.lock().unwrap().get_mut(&user_id) {
                s.last_alert_scan_at = Some(scanned_at);
            }
            Ok(())
        }

        async fn list_watchlist_rules(
            &self,
            user_id: Uuid,
        ) -> Result<Vec<WatchlistRuleRecord>, CoreError> {
            Ok(self
                .watchlist
                .lock()
                .unwrap()
                .iter()
                .filter(|w| w.user_id == user_id)
                .cloned()
                .collect())
        }

        async fn create_watchlist_rule(
            &self,
            user_id: Uuid,
            cmd: UpsertWatchlistRuleCmd,
        ) -> Result<WatchlistRuleRecord, CoreError> {
            let rec = WatchlistRuleRecord {
                id: Uuid::new_v4(),
                user_id,
                name: cmd.name,
                keywords: cmd.keywords,
                exclude_keywords: cmd.exclude_keywords,
                priority: cmd.priority,
                notify_immediately: cmd.notify_immediately,
                enabled: cmd.enabled,
                created_at: Utc::now(),
            };
            self.watchlist.lock().unwrap().push(rec.clone());
            Ok(rec)
        }

        async fn update_watchlist_rule(
            &self,
            _user_id: Uuid,
            _rule_id: Uuid,
            _cmd: UpsertWatchlistRuleCmd,
        ) -> Result<WatchlistRuleRecord, CoreError> {
            Err(CoreError::NotFound("not implemented".to_owned()))
        }

        async fn delete_watchlist_rule(
            &self,
            _user_id: Uuid,
            _rule_id: Uuid,
        ) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn scheduler_tasks_shut_down_cleanly_on_watch_signal() {
        use std::sync::Arc;
        use std::time::Duration;

        use digest_core::cache::InMemoryCacheCoordinator;
        use digest_core::collectors::{CollectorContext, CollectorRegistry};
        use digest_core::config::NotifierTuning;
        use digest_core::http::shared_http_client;
        use digest_core::llm::{IdempotentLlmRunner, OpenAiCompatProvider};
        use digest_core::notifiers::NotifierRegistry;
        use digest_core::pipeline::ItemProcessingPipeline;
        use digest_core::renderer::RendererRegistry;
        use digest_core::ssrf::SsrfGuard;
        use digest_core::traits::StructuredLlmProvider;
        use tokio::sync::watch;

        use crate::config::ServerConfig;
        use crate::scheduler::ComposerDeps;
        use crate::sse_hub::SseHub;

        let cfg = ServerConfig::default();
        let client = shared_http_client(None, &cfg.core.collector).unwrap();
        let ssrf = SsrfGuard::from_config(&cfg.core);
        let cache: Arc<dyn digest_core::cache::CacheCoordinator> =
            Arc::new(InMemoryCacheCoordinator::new());
        let collectors = Arc::new(CollectorRegistry::new(CollectorContext::new(
            client.clone(),
            ssrf.clone(),
            cfg.core.collector.clone(),
            cfg.core.resources.clone(),
        )));
        let provider: Arc<dyn StructuredLlmProvider> =
            Arc::new(OpenAiCompatProvider::new(client.clone(), ssrf.clone()));
        let llm_runner = Arc::new(IdempotentLlmRunner::new(Arc::clone(&cache), provider));
        let notifiers = Arc::new(NotifierRegistry::with_defaults(
            client,
            ssrf,
            NotifierTuning::default(),
        ));
        let mock = MockRepos::new();

        let deps = ComposerDeps {
            config: Arc::new(cfg),
            repos: mock.repositories(),
            cipher: Arc::new(EnvelopeCipher::from_bytes([0x55u8; 32])),
            cache,
            collectors,
            pipeline: Arc::new(ItemProcessingPipeline::new()),
            llm_runner,
            renderer: Arc::new(RendererRegistry::new()),
            notifiers,
            sse: Arc::new(SseHub::new()),
        };

        let (tx, rx) = watch::channel(false);
        let handles = super::spawn_scheduler_tasks(deps, rx);
        tokio::time::sleep(Duration::from_millis(25)).await;
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), handles.join_all())
            .await
            .unwrap();
    }
}
