//! In-memory repository implementation for testing and lightweight runs.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use digest_core::constants::{
    DEFAULT_ALERT_INTERVAL_MINUTES, DEFAULT_DAILY_TIME, DEFAULT_TIMEZONE,
    SECRET_MASK_CONFIGURED_PLACEHOLDER,
};
use digest_core::crypto::EnvelopeCipher;
use digest_core::domain::{
    AlertPriority, DeliveryStatus, RunStatus, SourceScope, WatchlistRuleInput,
};
use digest_core::error::CoreError;
use digest_core::presets::find_preset_by_key;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::dto::{
    BookmarkView, CanonicalItemRecord, CanonicalSourceRecord, CreateChannelRecord,
    CreateDeliveryLogCmd, CreateRunCmd, DailyDigestDetail, DailyDigestSummary, DeliveryLogRecord,
    DigestRunRecord, DigestRunView, FinishRunCmd, ItemSearchQuery, ItemView, NewCanonicalItem,
    NewUserItemMatch, NotificationChannelRecord, OnboardingFlags, PageRequest, PagedResult,
    PendingAlertItem, RunStatsView, SaveDigestCmd, SourceDailyDigestView, SourceFetchStateUpdate,
    SourceHealthStat, SubscribeSourceCmd, UpdateChannelRecord, UpdateScheduleCmd, UpdateSourceCmd,
    UpsertAiConfigRecord, UpsertUserCmd, UpsertWatchlistRuleCmd, UserAiConfigRecord, UserRecord,
    UserScheduleRecord, UserSourceRecord, UserSourceView, WatchlistRuleRecord,
};
use super::{DigestRepo, Repositories, RunRepo, SettingsRepo, SourceRepo, UserRepo};

#[derive(Debug, Clone)]
struct StoredSubscription {
    id: Uuid,
    user_id: Uuid,
    canonical_source_id: Uuid,
    custom_name: String,
    category: String,
    enabled: bool,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct StoredItemMatch {
    user_id: Uuid,
    item_id: Uuid,
    matched_keywords: Vec<String>,
    priority: AlertPriority,
    importance_score: i32,
    alerted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct StoredDailyDigest {
    id: Uuid,
    user_id: Uuid,
    digest_date: NaiveDate,
    input_fingerprint: String,
    title: String,
    overview_markdown: String,
    overview_html: String,
    sections: Vec<digest_core::domain::DigestSection>,
    item_count: i32,
    highlighted_count: i32,
    prompt_tokens: i32,
    completion_tokens: i32,
    model_used: String,
    degraded_fallback: bool,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct StoredSourceDailyDigest {
    id: Uuid,
    user_id: Uuid,
    digest_id: Option<Uuid>,
    canonical_source_id: Uuid,
    digest_date: NaiveDate,
    input_fingerprint: String,
    summary_markdown: String,
    key_points: Vec<String>,
    item_count: i32,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct StoredBookmark {
    id: Uuid,
    item_id: Uuid,
    note: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
struct InMemoryState {
    users: HashMap<Uuid, UserRecord>,
    users_by_sub: HashMap<String, Uuid>,
    ai_configs: HashMap<Uuid, UserAiConfigRecord>,
    canonical_sources: HashMap<Uuid, CanonicalSourceRecord>,
    /// Fallback non-secret handle_or_url stored for tests when `cipher` is `None`.
    source_handles: HashMap<Uuid, String>,
    subscriptions: HashMap<Uuid, StoredSubscription>,
    canonical_items: HashMap<Uuid, CanonicalItemRecord>,
    item_matches: HashMap<(Uuid, Uuid), StoredItemMatch>,
    daily_digests: HashMap<(Uuid, NaiveDate), StoredDailyDigest>,
    source_daily_digests: HashMap<(Uuid, Uuid, NaiveDate), StoredSourceDailyDigest>,
    channels: HashMap<Uuid, NotificationChannelRecord>,
    schedules: HashMap<Uuid, UserScheduleRecord>,
    watchlist_rules: HashMap<Uuid, WatchlistRuleRecord>,
    runs: HashMap<Uuid, DigestRunRecord>,
    deliveries: HashMap<Uuid, DeliveryLogRecord>,
    bookmarks: HashMap<(Uuid, Uuid), StoredBookmark>,
}

/// Thread-safe in-memory repository store implementing all 5 repository traits
/// with PostgreSQL-equivalent isolation, deduplication, search, and retention semantics.
#[derive(Clone, Default)]
pub struct InMemoryRepoStore {
    state: Arc<RwLock<InMemoryState>>,
    cipher: Option<Arc<EnvelopeCipher>>,
}

impl InMemoryRepoStore {
    /// Creates an empty `InMemoryRepoStore`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an `InMemoryRepoStore` with an `EnvelopeCipher` for decrypting `config_json_enc`.
    #[must_use]
    pub fn with_cipher(cipher: Arc<EnvelopeCipher>) -> Self {
        Self {
            state: Arc::new(RwLock::new(InMemoryState::default())),
            cipher: Some(cipher),
        }
    }

    /// Bundles a fresh `InMemoryRepoStore` into `Repositories`.
    #[must_use]
    pub fn repositories() -> Repositories {
        Arc::new(Self::new()).into_repositories()
    }

    /// Bundles a fresh `InMemoryRepoStore` with `EnvelopeCipher` into `Repositories`.
    #[must_use]
    pub fn repositories_with_cipher(cipher: Arc<EnvelopeCipher>) -> Repositories {
        Arc::new(Self::with_cipher(cipher)).into_repositories()
    }

    /// Converts an `Arc<InMemoryRepoStore>` into a `Repositories` trait-object bundle.
    #[must_use]
    pub fn into_repositories(self: Arc<Self>) -> Repositories {
        Repositories {
            users: Arc::clone(&self) as Arc<dyn UserRepo>,
            sources: Arc::clone(&self) as Arc<dyn SourceRepo>,
            digests: Arc::clone(&self) as Arc<dyn DigestRepo>,
            runs: Arc::clone(&self) as Arc<dyn RunRepo>,
            settings: self as Arc<dyn SettingsRepo>,
        }
    }

    fn build_source_view(
        &self,
        rec: &UserSourceRecord,
        fallback_handle: Option<&str>,
    ) -> Result<UserSourceView, CoreError> {
        if let Some(ref cipher) = self.cipher {
            if rec.config_json_enc.is_some() {
                return rec.to_view(cipher);
            }
            if let Some(preset) = find_preset_by_key(&rec.canonical_key) {
                return Ok(UserSourceView {
                    id: rec.canonical_source_id,
                    subscription_id: rec.subscription_id,
                    source_type: rec.source_type,
                    scope: rec.scope,
                    canonical_key: rec.canonical_key.clone(),
                    custom_name: rec.custom_name.clone(),
                    default_name: rec.default_name.clone(),
                    category: rec.category.clone(),
                    handle_or_url: preset.handle_or_url,
                    enabled: rec.enabled,
                    has_credentials: false,
                    config_hint: None,
                    last_fetched_at: rec.last_fetched_at,
                    next_poll_at: rec.next_poll_at,
                    last_status: rec.last_status,
                    last_error: rec.last_error.clone(),
                    consecutive_failures: rec.consecutive_failures,
                    created_at: rec.created_at,
                });
            }
            let mut view = rec.to_view(cipher)?;
            if let Some(handle) = fallback_handle.filter(|h| !h.trim().is_empty()) {
                view.handle_or_url = handle.to_owned();
            }
            return Ok(view);
        }

        let handle_or_url = fallback_handle
            .filter(|h| !h.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| find_preset_by_key(&rec.canonical_key).map(|p| p.handle_or_url))
            .unwrap_or_else(|| rec.canonical_key.clone());
        let has_credentials = rec
            .config_json_enc
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
        let config_hint = if has_credentials {
            Some(SECRET_MASK_CONFIGURED_PLACEHOLDER.to_owned())
        } else {
            None
        };

        Ok(UserSourceView {
            id: rec.canonical_source_id,
            subscription_id: rec.subscription_id,
            source_type: rec.source_type,
            scope: rec.scope,
            canonical_key: rec.canonical_key.clone(),
            custom_name: rec.custom_name.clone(),
            default_name: rec.default_name.clone(),
            category: rec.category.clone(),
            handle_or_url,
            enabled: rec.enabled,
            has_credentials,
            config_hint,
            last_fetched_at: rec.last_fetched_at,
            next_poll_at: rec.next_poll_at,
            last_status: rec.last_status,
            last_error: rec.last_error.clone(),
            consecutive_failures: rec.consecutive_failures,
            created_at: rec.created_at,
        })
    }
}

/// Module-level helper returning an `InMemoryRepoStore`-backed `Repositories` bundle.
#[must_use]
pub fn repositories() -> Repositories {
    InMemoryRepoStore::repositories()
}

fn default_schedule(user_id: Uuid) -> UserScheduleRecord {
    UserScheduleRecord {
        user_id,
        timezone: DEFAULT_TIMEZONE.to_owned(),
        daily_time: DEFAULT_DAILY_TIME.to_owned(),
        email_enabled: true,
        send_when_empty: false,
        alert_scan_enabled: true,
        alert_interval_minutes: i32::try_from(DEFAULT_ALERT_INTERVAL_MINUTES).unwrap_or(60),
        last_daily_run_date: None,
        last_alert_scan_at: None,
    }
}

fn join_user_source_record(
    sub: &StoredSubscription,
    src: &CanonicalSourceRecord,
) -> UserSourceRecord {
    UserSourceRecord {
        subscription_id: sub.id,
        canonical_source_id: src.id,
        user_id: sub.user_id,
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
    }
}

fn resolve_user_subscription(
    state: &InMemoryState,
    user_id: Uuid,
    id_or_sub: Uuid,
) -> Option<(&StoredSubscription, &CanonicalSourceRecord)> {
    state.subscriptions.values().find_map(|sub| {
        if sub.user_id != user_id {
            return None;
        }
        if sub.canonical_source_id != id_or_sub && sub.id != id_or_sub {
            return None;
        }
        let src = state.canonical_sources.get(&sub.canonical_source_id)?;
        if src.scope == SourceScope::UserPrivate && src.scope_owner_id != Some(user_id) {
            return None;
        }
        Some((sub, src))
    })
}

fn build_item_view(
    state: &InMemoryState,
    user_id: Uuid,
    sub: &StoredSubscription,
    item: &CanonicalItemRecord,
) -> ItemView {
    let matched = state.item_matches.get(&(user_id, item.id));
    let bookmark = state.bookmarks.get(&(user_id, item.id));
    ItemView {
        id: item.id,
        source_id: sub.canonical_source_id,
        source_name: sub.custom_name.clone(),
        category: sub.category.clone(),
        external_id: item.external_id.clone(),
        canonical_url: item.canonical_url.clone(),
        title: item.title.clone(),
        author: item.author.clone(),
        summary: item.summary_raw.clone(),
        body_pruned: item.body_pruned,
        published_at: item.published_at,
        fetched_at: item.fetched_at,
        matched_keywords: matched
            .map(|m| m.matched_keywords.clone())
            .unwrap_or_default(),
        priority: matched.map_or(AlertPriority::Normal, |m| m.priority),
        importance_score: matched.map_or(0, |m| m.importance_score),
        bookmarked: bookmark.is_some(),
        bookmark_note: bookmark.and_then(|b| b.note.clone()),
    }
}

fn matches_fts_query(item: &CanonicalItemRecord, raw_query: &str) -> bool {
    let tokens: Vec<String> = raw_query
        .split_whitespace()
        .map(|tok| tok.trim_matches('"').trim())
        .filter(|tok| !tok.is_empty() && !tok.eq_ignore_ascii_case("OR"))
        .map(|tok| tok.to_ascii_lowercase())
        .collect();

    if tokens.is_empty() {
        return false;
    }

    let mut haystack = item.title.to_ascii_lowercase();
    if let Some(ref author) = item.author {
        haystack.push(' ');
        haystack.push_str(&author.to_ascii_lowercase());
    }
    if !item.body_pruned {
        if let Some(ref summary) = item.summary_raw {
            haystack.push(' ');
            haystack.push_str(&summary.to_ascii_lowercase());
        }
    }

    let has_or = raw_query
        .split_whitespace()
        .any(|tok| tok.eq_ignore_ascii_case("OR"));
    if has_or {
        tokens.iter().any(|tok| haystack.contains(tok))
    } else {
        tokens.iter().all(|tok| haystack.contains(tok))
    }
}

fn to_daily_detail(rec: &StoredDailyDigest) -> DailyDigestDetail {
    DailyDigestDetail {
        id: rec.id,
        digest_date: rec.digest_date,
        input_fingerprint: rec.input_fingerprint.clone(),
        title: rec.title.clone(),
        overview_markdown: rec.overview_markdown.clone(),
        overview_html: rec.overview_html.clone(),
        sections: rec.sections.clone(),
        item_count: rec.item_count,
        highlighted_count: rec.highlighted_count,
        prompt_tokens: rec.prompt_tokens,
        completion_tokens: rec.completion_tokens,
        model_used: rec.model_used.clone(),
        degraded_fallback: rec.degraded_fallback,
        created_at: rec.created_at,
    }
}

fn paginate_vec<T>(mut items: Vec<T>, page: PageRequest) -> PagedResult<T> {
    let total = u64::try_from(items.len()).unwrap_or(u64::MAX);
    let offset = page.offset();
    if offset >= items.len() {
        return PagedResult::new(Vec::new(), page, total);
    }
    let slice: Vec<T> = items.drain(offset..).take(page.limit()).collect();
    PagedResult::new(slice, page, total)
}

#[async_trait]
impl UserRepo for InMemoryRepoStore {
    async fn upsert_oidc_user(&self, cmd: UpsertUserCmd) -> Result<UserRecord, CoreError> {
        let oidc_sub = cmd.oidc_sub.trim();
        if oidc_sub.is_empty() {
            return Err(CoreError::InvalidInput(
                "oidc_sub must not be empty".to_owned(),
            ));
        }

        let mut state = self.state.write().await;
        let now = Utc::now();
        let user_id = if let Some(&existing_id) = state.users_by_sub.get(oidc_sub) {
            if let Some(existing) = state.users.get_mut(&existing_id) {
                if cmd.email.is_some() {
                    existing.email = cmd.email;
                }
                if cmd.display_name.is_some() {
                    existing.display_name = cmd.display_name;
                }
                existing.last_login_at = Some(now);
            }
            existing_id
        } else {
            let new_id = Uuid::new_v4();
            let role = if cmd.role.trim().is_empty() {
                crate::constants::DEFAULT_USER_ROLE.to_owned()
            } else {
                cmd.role.trim().to_owned()
            };
            let record = UserRecord {
                id: new_id,
                oidc_sub: oidc_sub.to_owned(),
                email: cmd.email,
                display_name: cmd.display_name,
                role,
                created_at: now,
                last_login_at: Some(now),
            };
            state.users_by_sub.insert(oidc_sub.to_owned(), new_id);
            state.users.insert(new_id, record);
            new_id
        };

        state
            .schedules
            .entry(user_id)
            .or_insert_with(|| default_schedule(user_id));

        state
            .users
            .get(&user_id)
            .cloned()
            .ok_or_else(|| CoreError::Internal("upserted user missing from state".to_owned()))
    }

    async fn get_user_by_id(&self, user_id: Uuid) -> Result<Option<UserRecord>, CoreError> {
        let state = self.state.read().await;
        Ok(state.users.get(&user_id).cloned())
    }

    async fn get_onboarding_flags(&self, user_id: Uuid) -> Result<OnboardingFlags, CoreError> {
        let state = self.state.read().await;
        Ok(OnboardingFlags {
            has_ai_config: state.ai_configs.contains_key(&user_id),
            has_sources: state.subscriptions.values().any(|s| s.user_id == user_id),
            has_channels: state.channels.values().any(|c| c.user_id == user_id),
            has_schedule: state.schedules.contains_key(&user_id),
        })
    }
}

#[async_trait]
impl SourceRepo for InMemoryRepoStore {
    async fn create_or_subscribe(
        &self,
        user_id: Uuid,
        cmd: SubscribeSourceCmd,
    ) -> Result<UserSourceView, CoreError> {
        let canonical_key = cmd.canonical_key.trim();
        let custom_name = cmd.custom_name.trim();
        let default_name = cmd.default_name.trim();
        let category = cmd.category.trim();
        if canonical_key.is_empty()
            || custom_name.is_empty()
            || default_name.is_empty()
            || category.is_empty()
        {
            return Err(CoreError::InvalidInput(
                "source canonicalKey, customName, defaultName, and category must be non-empty"
                    .to_owned(),
            ));
        }

        let scope_owner_id = cmd.scope_owner_id(user_id);
        let now = Utc::now();
        let mut state = self.state.write().await;

        let existing_source_id = state.canonical_sources.values().find_map(|src| {
            if src.source_type == cmd.source_type
                && src.canonical_key == canonical_key
                && src.scope_owner_id == scope_owner_id
            {
                Some(src.id)
            } else {
                None
            }
        });

        let canonical_source_id = if let Some(src_id) = existing_source_id {
            if let Some(src) = state.canonical_sources.get_mut(&src_id) {
                if src.scope == SourceScope::UserPrivate {
                    if let Some(ref enc) = cmd.config_json_enc {
                        src.config_json_enc = Some(enc.clone());
                    }
                } else if src.config_json_enc.is_none() && cmd.config_json_enc.is_some() {
                    src.config_json_enc = cmd.config_json_enc.clone();
                }
            }
            if !cmd.handle_or_url.trim().is_empty() {
                state
                    .source_handles
                    .entry(src_id)
                    .or_insert_with(|| cmd.handle_or_url.trim().to_owned());
            }
            src_id
        } else {
            let src_id = Uuid::new_v4();
            let src = CanonicalSourceRecord {
                id: src_id,
                source_type: cmd.source_type,
                canonical_key: canonical_key.to_owned(),
                scope: cmd.scope,
                scope_owner_id,
                default_name: default_name.to_owned(),
                config_json_enc: cmd.config_json_enc.clone(),
                etag: None,
                last_modified: None,
                last_fetched_at: None,
                next_poll_at: None,
                last_status: None,
                last_error: None,
                consecutive_failures: 0,
                created_at: now,
            };
            state.canonical_sources.insert(src_id, src);
            if !cmd.handle_or_url.trim().is_empty() {
                state
                    .source_handles
                    .insert(src_id, cmd.handle_or_url.trim().to_owned());
            }
            src_id
        };

        let existing_sub_id = state.subscriptions.values().find_map(|sub| {
            if sub.user_id == user_id && sub.canonical_source_id == canonical_source_id {
                Some(sub.id)
            } else {
                None
            }
        });

        let sub_id = if let Some(sid) = existing_sub_id {
            if let Some(sub) = state.subscriptions.get_mut(&sid) {
                sub.custom_name = custom_name.to_owned();
                sub.category = category.to_owned();
                sub.enabled = cmd.enabled;
            }
            sid
        } else {
            let sid = Uuid::new_v4();
            state.subscriptions.insert(
                sid,
                StoredSubscription {
                    id: sid,
                    user_id,
                    canonical_source_id,
                    custom_name: custom_name.to_owned(),
                    category: category.to_owned(),
                    enabled: cmd.enabled,
                    created_at: now,
                },
            );
            sid
        };

        let sub = state
            .subscriptions
            .get(&sub_id)
            .ok_or_else(|| CoreError::Internal("subscription missing after insert".to_owned()))?;
        let src = state
            .canonical_sources
            .get(&canonical_source_id)
            .ok_or_else(|| CoreError::Internal("canonical source missing".to_owned()))?;
        let rec = join_user_source_record(sub, src);
        let fallback = state
            .source_handles
            .get(&canonical_source_id)
            .map(String::as_str);
        self.build_source_view(&rec, fallback)
    }

    async fn list_user_sources(&self, user_id: Uuid) -> Result<Vec<UserSourceView>, CoreError> {
        let state = self.state.read().await;
        let mut records: Vec<(UserSourceRecord, Option<String>)> = state
            .subscriptions
            .values()
            .filter(|sub| sub.user_id == user_id)
            .filter_map(|sub| {
                let src = state.canonical_sources.get(&sub.canonical_source_id)?;
                if src.scope == SourceScope::UserPrivate && src.scope_owner_id != Some(user_id) {
                    return None;
                }
                let fallback = state.source_handles.get(&src.id).cloned();
                Some((join_user_source_record(sub, src), fallback))
            })
            .collect();

        records.sort_by(|a, b| {
            a.0.created_at
                .cmp(&b.0.created_at)
                .then_with(|| a.0.subscription_id.cmp(&b.0.subscription_id))
        });

        records
            .iter()
            .map(|(rec, fallback)| self.build_source_view(rec, fallback.as_deref()))
            .collect()
    }

    async fn get_user_source(
        &self,
        user_id: Uuid,
        id_or_sub: Uuid,
    ) -> Result<Option<UserSourceRecord>, CoreError> {
        let state = self.state.read().await;
        Ok(resolve_user_subscription(&state, user_id, id_or_sub)
            .map(|(sub, src)| join_user_source_record(sub, src)))
    }

    async fn update_user_subscription(
        &self,
        user_id: Uuid,
        id_or_sub: Uuid,
        cmd: UpdateSourceCmd,
    ) -> Result<UserSourceView, CoreError> {
        let mut state = self.state.write().await;
        let (sub_id, src_id, scope, scope_owner_id) =
            resolve_user_subscription(&state, user_id, id_or_sub)
                .map(|(sub, src)| (sub.id, src.id, src.scope, src.scope_owner_id))
                .ok_or_else(|| CoreError::NotFound("source subscription not found".to_owned()))?;

        if scope == SourceScope::SharedPublic
            && (cmd.config_json_enc.is_some() || cmd.handle_or_url.is_some())
        {
            return Err(CoreError::Forbidden(
                "shared_public canonical source config cannot be modified per-user".to_owned(),
            ));
        }

        if let Some(ref name) = cmd.custom_name {
            if name.trim().is_empty() {
                return Err(CoreError::InvalidInput(
                    "customName cannot be empty".to_owned(),
                ));
            }
        }
        if let Some(ref cat) = cmd.category {
            if cat.trim().is_empty() {
                return Err(CoreError::InvalidInput(
                    "category cannot be empty".to_owned(),
                ));
            }
        }

        if let Some(sub) = state.subscriptions.get_mut(&sub_id) {
            if let Some(ref name) = cmd.custom_name {
                sub.custom_name = name.trim().to_owned();
            }
            if let Some(ref cat) = cmd.category {
                sub.category = cat.trim().to_owned();
            }
            if let Some(enabled) = cmd.enabled {
                sub.enabled = enabled;
            }
        }

        if scope == SourceScope::UserPrivate && scope_owner_id == Some(user_id) {
            if let Some(ref enc) = cmd.config_json_enc {
                if let Some(src) = state.canonical_sources.get_mut(&src_id) {
                    src.config_json_enc = Some(enc.clone());
                }
            }
            if let Some(ref handle) = cmd.handle_or_url {
                if !handle.trim().is_empty() {
                    state
                        .source_handles
                        .insert(src_id, handle.trim().to_owned());
                }
            }
        }

        let sub = state
            .subscriptions
            .get(&sub_id)
            .ok_or_else(|| CoreError::NotFound("source subscription not found".to_owned()))?;
        let src = state
            .canonical_sources
            .get(&src_id)
            .ok_or_else(|| CoreError::NotFound("canonical source not found".to_owned()))?;
        let rec = join_user_source_record(sub, src);
        let fallback = state.source_handles.get(&src_id).map(String::as_str);
        self.build_source_view(&rec, fallback)
    }

    async fn delete_user_subscription(
        &self,
        user_id: Uuid,
        id_or_sub: Uuid,
    ) -> Result<(), CoreError> {
        let mut state = self.state.write().await;
        let (sub_id, src_id, scope, scope_owner_id) =
            resolve_user_subscription(&state, user_id, id_or_sub)
                .map(|(sub, src)| (sub.id, src.id, src.scope, src.scope_owner_id))
                .ok_or_else(|| CoreError::NotFound("source subscription not found".to_owned()))?;

        state.subscriptions.remove(&sub_id);

        if scope == SourceScope::UserPrivate && scope_owner_id == Some(user_id) {
            state.canonical_sources.remove(&src_id);
            state.source_handles.remove(&src_id);
            let removed_item_ids: Vec<Uuid> = state
                .canonical_items
                .values()
                .filter(|it| it.canonical_source_id == src_id)
                .map(|it| it.id)
                .collect();
            for item_id in removed_item_ids {
                state.canonical_items.remove(&item_id);
                state.item_matches.retain(|(_, iid), _| *iid != item_id);
                state.bookmarks.retain(|(_, iid), _| *iid != item_id);
            }
            state
                .source_daily_digests
                .retain(|(_, sid, _), _| *sid != src_id);
        }

        Ok(())
    }

    async fn list_pollable_sources(
        &self,
        scope: Option<SourceScope>,
        due_before: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<CanonicalSourceRecord>, CoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let state = self.state.read().await;
        let enabled_source_ids: HashSet<Uuid> = state
            .subscriptions
            .values()
            .filter(|sub| sub.enabled)
            .map(|sub| sub.canonical_source_id)
            .collect();

        let mut sources: Vec<CanonicalSourceRecord> = state
            .canonical_sources
            .values()
            .filter(|src| enabled_source_ids.contains(&src.id))
            .filter(|src| scope.is_none_or(|s| src.scope == s))
            .filter(|src| src.next_poll_at.is_none_or(|t| t <= due_before))
            .cloned()
            .collect();

        sources.sort_by(|a, b| {
            a.next_poll_at
                .cmp(&b.next_poll_at)
                .then_with(|| a.last_fetched_at.cmp(&b.last_fetched_at))
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        sources.truncate(limit);
        Ok(sources)
    }

    async fn record_fetch_outcome(
        &self,
        source_id: Uuid,
        update: SourceFetchStateUpdate,
    ) -> Result<(), CoreError> {
        let mut state = self.state.write().await;
        let src = state
            .canonical_sources
            .get_mut(&source_id)
            .ok_or_else(|| CoreError::NotFound("canonical source not found".to_owned()))?;
        src.last_status = Some(update.outcome);
        if update.etag.is_some() {
            src.etag = update.etag;
        }
        if update.last_modified.is_some() {
            src.last_modified = update.last_modified;
        }
        src.last_fetched_at = Some(update.fetched_at);
        src.next_poll_at = update.next_poll_at;
        src.last_error = update.last_error;
        src.consecutive_failures = update.consecutive_failures;
        Ok(())
    }

    async fn upsert_canonical_items(
        &self,
        source_id: Uuid,
        items: &[NewCanonicalItem],
    ) -> Result<Vec<CanonicalItemRecord>, CoreError> {
        let mut state = self.state.write().await;
        if !state.canonical_sources.contains_key(&source_id) {
            return Err(CoreError::NotFound("canonical source not found".to_owned()));
        }
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let mut inserted = Vec::new();
        let now = Utc::now();

        for item in items {
            let duplicate = state.canonical_items.values().any(|existing| {
                existing.canonical_source_id == source_id
                    && (existing.canonical_url == item.canonical_url
                        || existing.content_hash == item.content_hash)
            });
            if duplicate {
                continue;
            }
            let record = CanonicalItemRecord {
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
                fetched_at: now,
            };
            state.canonical_items.insert(record.id, record.clone());
            inserted.push(record);
        }

        Ok(inserted)
    }

    async fn list_recent_source_items(
        &self,
        source_id: Uuid,
        since: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<CanonicalItemRecord>, CoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let state = self.state.read().await;
        let mut items: Vec<CanonicalItemRecord> = state
            .canonical_items
            .values()
            .filter(|it| {
                it.canonical_source_id == source_id
                    && it.published_at.unwrap_or(it.fetched_at) >= since
            })
            .cloned()
            .collect();

        items.sort_by(|a, b| {
            b.published_at
                .unwrap_or(b.fetched_at)
                .cmp(&a.published_at.unwrap_or(a.fetched_at))
                .then_with(|| b.fetched_at.cmp(&a.fetched_at))
        });
        items.truncate(limit);
        Ok(items)
    }

    async fn list_source_items(
        &self,
        user_id: Uuid,
        source_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<ItemView>, CoreError> {
        let state = self.state.read().await;
        let (sub, _) = resolve_user_subscription(&state, user_id, source_id)
            .ok_or_else(|| CoreError::NotFound("source subscription not found".to_owned()))?;

        let mut items: Vec<(DateTime<Utc>, DateTime<Utc>, Uuid, ItemView)> = state
            .canonical_items
            .values()
            .filter(|it| it.canonical_source_id == sub.canonical_source_id)
            .map(|it| {
                (
                    it.published_at.unwrap_or(it.fetched_at),
                    it.fetched_at,
                    it.id,
                    build_item_view(&state, user_id, sub, it),
                )
            })
            .collect();

        items.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| b.2.cmp(&a.2))
        });
        let views: Vec<ItemView> = items.into_iter().map(|entry| entry.3).collect();
        Ok(paginate_vec(views, page))
    }

    async fn search_user_items(
        &self,
        user_id: Uuid,
        query: ItemSearchQuery,
    ) -> Result<PagedResult<ItemView>, CoreError> {
        let state = self.state.read().await;

        let user_subs: HashMap<Uuid, &StoredSubscription> = state
            .subscriptions
            .values()
            .filter(|sub| sub.user_id == user_id)
            .filter(|sub| {
                state
                    .canonical_sources
                    .get(&sub.canonical_source_id)
                    .is_some_and(|src| {
                        src.scope == SourceScope::SharedPublic
                            || src.scope_owner_id == Some(user_id)
                    })
            })
            .map(|sub| (sub.canonical_source_id, sub))
            .collect();

        let q_filter = query.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let kw_filter = query
            .keyword
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);
        let date_from_dt = query
            .date_from
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| dt.and_utc());
        let date_to_excl_dt = query
            .date_to
            .and_then(|d| d.succ_opt())
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| dt.and_utc());

        let mut matched_rows: Vec<(DateTime<Utc>, DateTime<Utc>, Uuid, ItemView)> = state
            .canonical_items
            .values()
            .filter_map(|item| {
                let sub = user_subs.get(&item.canonical_source_id)?;

                if let Some(src_filter) = query.source_id {
                    if sub.canonical_source_id != src_filter && sub.id != src_filter {
                        return None;
                    }
                }

                let effective_ts = item.published_at.unwrap_or(item.fetched_at);
                if let Some(from_dt) = date_from_dt {
                    if effective_ts < from_dt {
                        return None;
                    }
                }
                if let Some(to_dt) = date_to_excl_dt {
                    if effective_ts >= to_dt {
                        return None;
                    }
                }

                if let Some(q_str) = q_filter {
                    if !matches_fts_query(item, q_str) {
                        return None;
                    }
                }

                let view = build_item_view(&state, user_id, sub, item);

                if let Some(ref kw) = kw_filter {
                    let has_kw = view
                        .matched_keywords
                        .iter()
                        .any(|m| m.to_ascii_lowercase() == *kw);
                    if !has_kw {
                        return None;
                    }
                }

                if let Some(bookmarked) = query.bookmarked {
                    if view.bookmarked != bookmarked {
                        return None;
                    }
                }

                Some((effective_ts, item.fetched_at, item.id, view))
            })
            .collect();

        matched_rows.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| b.2.cmp(&a.2))
        });

        let views: Vec<ItemView> = matched_rows.into_iter().map(|entry| entry.3).collect();
        Ok(paginate_vec(views, query.page))
    }

    async fn upsert_user_item_matches(
        &self,
        user_id: Uuid,
        matches: &[NewUserItemMatch],
    ) -> Result<(), CoreError> {
        let mut state = self.state.write().await;
        for m in matches {
            let entry = state
                .item_matches
                .entry((user_id, m.item_id))
                .or_insert_with(|| StoredItemMatch {
                    user_id,
                    item_id: m.item_id,
                    matched_keywords: Vec::new(),
                    priority: m.priority,
                    importance_score: 0,
                    alerted_at: None,
                });
            entry.matched_keywords = m.matched_keywords.clone();
            entry.priority = m.priority;
            entry.importance_score = m.importance_score;
        }
        Ok(())
    }

    async fn list_pending_urgent_alerts(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<PendingAlertItem>, CoreError> {
        let state = self.state.read().await;
        let user_subs: HashMap<Uuid, &StoredSubscription> = state
            .subscriptions
            .values()
            .filter(|sub| sub.user_id == user_id && sub.enabled)
            .filter(|sub| {
                state
                    .canonical_sources
                    .get(&sub.canonical_source_id)
                    .is_some_and(|src| {
                        src.scope == SourceScope::SharedPublic
                            || src.scope_owner_id == Some(user_id)
                    })
            })
            .map(|sub| (sub.canonical_source_id, sub))
            .collect();

        let mut alerts: Vec<(i32, DateTime<Utc>, PendingAlertItem)> = state
            .item_matches
            .values()
            .filter(|m| {
                m.user_id == user_id
                    && m.priority == AlertPriority::Urgent
                    && m.alerted_at.is_none()
            })
            .filter_map(|m| {
                let item = state.canonical_items.get(&m.item_id)?;
                let sub = user_subs.get(&item.canonical_source_id)?;
                let effective_ts = item.published_at.unwrap_or(item.fetched_at);
                Some((
                    m.importance_score,
                    effective_ts,
                    PendingAlertItem {
                        item_id: item.id,
                        source_id: sub.canonical_source_id,
                        source_name: sub.custom_name.clone(),
                        category: sub.category.clone(),
                        canonical_url: item.canonical_url.clone(),
                        title: item.title.clone(),
                        author: item.author.clone(),
                        summary: item.summary_raw.clone(),
                        published_at: item.published_at,
                        matched_keywords: m.matched_keywords.clone(),
                        priority: m.priority,
                        importance_score: m.importance_score,
                    },
                ))
            })
            .collect();

        alerts.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        Ok(alerts.into_iter().map(|entry| entry.2).collect())
    }

    async fn mark_items_alerted(
        &self,
        user_id: Uuid,
        item_ids: &[Uuid],
        alerted_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let mut state = self.state.write().await;
        for &item_id in item_ids {
            if let Some(m) = state.item_matches.get_mut(&(user_id, item_id)) {
                if m.alerted_at.is_none() {
                    m.alerted_at = Some(alerted_at);
                }
            }
        }
        Ok(())
    }

    async fn prune_unbookmarked_bodies(&self, cutoff: DateTime<Utc>) -> Result<u64, CoreError> {
        let mut state = self.state.write().await;
        let bookmarked_item_ids: HashSet<Uuid> = state
            .bookmarks
            .keys()
            .map(|(_, item_id)| *item_id)
            .collect();

        let mut pruned: u64 = 0;
        for item in state.canonical_items.values_mut() {
            if !item.body_pruned
                && item.fetched_at < cutoff
                && !bookmarked_item_ids.contains(&item.id)
            {
                item.summary_raw = None;
                item.body_pruned = true;
                pruned = pruned.saturating_add(1);
            }
        }
        Ok(pruned)
    }

    async fn upsert_bookmark(
        &self,
        user_id: Uuid,
        item_id: Uuid,
        note: Option<String>,
    ) -> Result<BookmarkView, CoreError> {
        let mut state = self.state.write().await;
        let has_access = state
            .canonical_items
            .get(&item_id)
            .and_then(|item| state.canonical_sources.get(&item.canonical_source_id))
            .is_some_and(|src| {
                src.scope == SourceScope::SharedPublic
                    || (src.scope == SourceScope::UserPrivate
                        && src.scope_owner_id == Some(user_id))
            });

        if !has_access {
            return Err(CoreError::NotFound("item not found".to_owned()));
        }

        let now = Utc::now();
        let entry = state
            .bookmarks
            .entry((user_id, item_id))
            .or_insert_with(|| StoredBookmark {
                id: Uuid::new_v4(),
                item_id,
                note: None,
                created_at: now,
            });
        entry.note = note;

        Ok(BookmarkView {
            id: entry.id,
            item_id: entry.item_id,
            note: entry.note.clone(),
            created_at: entry.created_at,
        })
    }

    async fn delete_bookmark(&self, user_id: Uuid, item_id: Uuid) -> Result<(), CoreError> {
        let mut state = self.state.write().await;
        if state.bookmarks.remove(&(user_id, item_id)).is_none() {
            return Err(CoreError::NotFound("bookmark not found".to_owned()));
        }
        Ok(())
    }
}

#[async_trait]
impl DigestRepo for InMemoryRepoStore {
    async fn list_daily_digests(
        &self,
        user_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<DailyDigestSummary>, CoreError> {
        let state = self.state.read().await;
        let mut rows: Vec<&StoredDailyDigest> = state
            .daily_digests
            .values()
            .filter(|d| d.user_id == user_id)
            .collect();
        rows.sort_by(|a, b| {
            b.digest_date
                .cmp(&a.digest_date)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });

        let summaries: Vec<DailyDigestSummary> = rows
            .into_iter()
            .map(|d| DailyDigestSummary {
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

        Ok(paginate_vec(summaries, page))
    }

    async fn get_daily_digest_by_date(
        &self,
        user_id: Uuid,
        date: NaiveDate,
    ) -> Result<Option<DailyDigestDetail>, CoreError> {
        let state = self.state.read().await;
        Ok(state
            .daily_digests
            .get(&(user_id, date))
            .map(to_daily_detail))
    }

    async fn find_daily_digest_by_fingerprint(
        &self,
        user_id: Uuid,
        date: NaiveDate,
        fingerprint: &str,
    ) -> Result<Option<DailyDigestDetail>, CoreError> {
        let state = self.state.read().await;
        Ok(state
            .daily_digests
            .get(&(user_id, date))
            .filter(|d| d.input_fingerprint == fingerprint)
            .map(to_daily_detail))
    }

    async fn save_daily_and_source_digests(
        &self,
        cmd: SaveDigestCmd,
    ) -> Result<DailyDigestDetail, CoreError> {
        let mut state = self.state.write().await;
        let now = Utc::now();
        let digest_id = state
            .daily_digests
            .get(&(cmd.user_id, cmd.digest_date))
            .map_or_else(Uuid::new_v4, |d| d.id);

        let stored = StoredDailyDigest {
            id: digest_id,
            user_id: cmd.user_id,
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
            created_at: now,
        };
        let detail = to_daily_detail(&stored);
        state
            .daily_digests
            .insert((cmd.user_id, cmd.digest_date), stored);

        for sd in cmd.source_digests {
            let sd_id = state
                .source_daily_digests
                .get(&(cmd.user_id, sd.canonical_source_id, sd.digest_date))
                .map_or_else(Uuid::new_v4, |existing| existing.id);
            state.source_daily_digests.insert(
                (cmd.user_id, sd.canonical_source_id, sd.digest_date),
                StoredSourceDailyDigest {
                    id: sd_id,
                    user_id: cmd.user_id,
                    digest_id: Some(digest_id),
                    canonical_source_id: sd.canonical_source_id,
                    digest_date: sd.digest_date,
                    input_fingerprint: sd.input_fingerprint,
                    summary_markdown: sd.summary_markdown,
                    key_points: sd.key_points,
                    item_count: sd.item_count,
                    created_at: now,
                },
            );
        }

        Ok(detail)
    }

    async fn list_source_daily_digests(
        &self,
        user_id: Uuid,
        source_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<SourceDailyDigestView>, CoreError> {
        let state = self.state.read().await;
        let (sub, src) = resolve_user_subscription(&state, user_id, source_id)
            .ok_or_else(|| CoreError::NotFound("source subscription not found".to_owned()))?;
        let canonical_source_id = src.id;
        let source_name = sub.custom_name.clone();

        let mut rows: Vec<&StoredSourceDailyDigest> = state
            .source_daily_digests
            .values()
            .filter(|sd| sd.user_id == user_id && sd.canonical_source_id == canonical_source_id)
            .collect();
        rows.sort_by(|a, b| {
            b.digest_date
                .cmp(&a.digest_date)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });

        let views: Vec<SourceDailyDigestView> = rows
            .into_iter()
            .map(|sd| SourceDailyDigestView {
                id: sd.id,
                digest_id: sd.digest_id,
                source_id: canonical_source_id,
                source_name: source_name.clone(),
                digest_date: sd.digest_date,
                input_fingerprint: sd.input_fingerprint.clone(),
                summary_markdown: sd.summary_markdown.clone(),
                key_points: sd.key_points.clone(),
                item_count: sd.item_count,
                created_at: sd.created_at,
            })
            .collect();

        Ok(paginate_vec(views, page))
    }
}

#[async_trait]
impl RunRepo for InMemoryRepoStore {
    async fn create_run(&self, cmd: CreateRunCmd) -> Result<DigestRunRecord, CoreError> {
        let mut state = self.state.write().await;
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
        state.runs.insert(rec.id, rec.clone());
        Ok(rec)
    }

    async fn finish_run(&self, run_id: Uuid, update: FinishRunCmd) -> Result<(), CoreError> {
        let mut state = self.state.write().await;
        let run = state
            .runs
            .get_mut(&run_id)
            .ok_or_else(|| CoreError::NotFound("digest run not found".to_owned()))?;
        run.status = update.status;
        run.finished_at = Some(update.finished_at);
        run.items_fetched = update.items_fetched;
        run.items_new = update.items_new;
        run.prompt_tokens = update.prompt_tokens;
        run.completion_tokens = update.completion_tokens;
        run.error_summary = update.error_summary;
        Ok(())
    }

    async fn list_runs_with_deliveries(
        &self,
        user_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<DigestRunView>, CoreError> {
        let state = self.state.read().await;
        let mut runs: Vec<&DigestRunRecord> = state
            .runs
            .values()
            .filter(|r| r.user_id == user_id)
            .collect();
        runs.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| b.id.cmp(&a.id))
        });

        let views: Vec<DigestRunView> = runs
            .into_iter()
            .map(|r| {
                let mut deliveries: Vec<DeliveryLogRecord> = state
                    .deliveries
                    .values()
                    .filter(|d| d.run_id == r.id && d.user_id == user_id)
                    .cloned()
                    .collect();
                deliveries.sort_by(|a, b| a.sent_at.cmp(&b.sent_at).then_with(|| a.id.cmp(&b.id)));
                DigestRunView {
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
                    deliveries,
                }
            })
            .collect();

        Ok(paginate_vec(views, page))
    }

    async fn record_delivery(
        &self,
        cmd: CreateDeliveryLogCmd,
    ) -> Result<DeliveryLogRecord, CoreError> {
        let mut state = self.state.write().await;
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
        state.deliveries.insert(rec.id, rec.clone());
        Ok(rec)
    }

    async fn get_delivery_by_id(
        &self,
        user_id: Uuid,
        delivery_id: Uuid,
    ) -> Result<Option<DeliveryLogRecord>, CoreError> {
        let state = self.state.read().await;
        Ok(state
            .deliveries
            .get(&delivery_id)
            .filter(|d| d.user_id == user_id)
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
        let mut state = self.state.write().await;
        let rec = state
            .deliveries
            .get_mut(&delivery_id)
            .filter(|d| d.user_id == user_id)
            .ok_or_else(|| CoreError::NotFound("delivery log not found".to_owned()))?;
        rec.status = status;
        rec.error_message = error_message;
        rec.sent_at = sent_at;
        Ok(rec.clone())
    }

    async fn get_run_stats(&self, user_id: Uuid) -> Result<RunStatsView, CoreError> {
        let state = self.state.read().await;
        let mut total_runs = 0_i64;
        let mut succeeded_runs = 0_i64;
        let mut failed_runs = 0_i64;
        let mut total_prompt_tokens = 0_i64;
        let mut total_completion_tokens = 0_i64;

        for run in state.runs.values().filter(|r| r.user_id == user_id) {
            total_runs += 1;
            match run.status {
                RunStatus::Succeeded => succeeded_runs += 1,
                RunStatus::Failed => failed_runs += 1,
                RunStatus::Running | RunStatus::Partial => {}
            }
            total_prompt_tokens += i64::from(run.prompt_tokens);
            total_completion_tokens += i64::from(run.completion_tokens);
        }

        let mut deliveries_sent = 0_i64;
        let mut deliveries_failed = 0_i64;
        for d in state.deliveries.values().filter(|d| d.user_id == user_id) {
            match d.status {
                DeliveryStatus::Sent => deliveries_sent += 1,
                DeliveryStatus::Failed => deliveries_failed += 1,
                DeliveryStatus::Pending => {}
            }
        }

        let mut subs: Vec<&StoredSubscription> = state
            .subscriptions
            .values()
            .filter(|s| s.user_id == user_id)
            .collect();
        subs.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });

        let source_health: Vec<SourceHealthStat> = subs
            .into_iter()
            .filter_map(|sub| {
                let src = state.canonical_sources.get(&sub.canonical_source_id)?;
                if src.scope == SourceScope::UserPrivate && src.scope_owner_id != Some(user_id) {
                    return None;
                }
                Some(SourceHealthStat {
                    source_id: src.id,
                    source_name: sub.custom_name.clone(),
                    source_type: src.source_type,
                    scope: src.scope,
                    enabled: sub.enabled,
                    last_status: src.last_status,
                    last_fetched_at: src.last_fetched_at,
                    consecutive_failures: src.consecutive_failures,
                    last_error: src.last_error.clone(),
                })
            })
            .collect();

        Ok(RunStatsView {
            total_runs,
            succeeded_runs,
            failed_runs,
            total_prompt_tokens,
            total_completion_tokens,
            deliveries_sent,
            deliveries_failed,
            source_health,
        })
    }
}

#[async_trait]
impl SettingsRepo for InMemoryRepoStore {
    async fn get_ai_config(&self, user_id: Uuid) -> Result<Option<UserAiConfigRecord>, CoreError> {
        let state = self.state.read().await;
        Ok(state.ai_configs.get(&user_id).cloned())
    }

    async fn upsert_ai_config(
        &self,
        user_id: Uuid,
        cmd: UpsertAiConfigRecord,
    ) -> Result<UserAiConfigRecord, CoreError> {
        let mut state = self.state.write().await;
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
        state.ai_configs.insert(user_id, rec.clone());
        Ok(rec)
    }

    async fn list_channels(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<NotificationChannelRecord>, CoreError> {
        let state = self.state.read().await;
        let mut channels: Vec<NotificationChannelRecord> = state
            .channels
            .values()
            .filter(|c| c.user_id == user_id)
            .cloned()
            .collect();
        channels.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(channels)
    }

    async fn get_channel(
        &self,
        user_id: Uuid,
        channel_id: Uuid,
    ) -> Result<Option<NotificationChannelRecord>, CoreError> {
        let state = self.state.read().await;
        Ok(state
            .channels
            .get(&channel_id)
            .filter(|c| c.user_id == user_id)
            .cloned())
    }

    async fn create_channel(
        &self,
        user_id: Uuid,
        cmd: CreateChannelRecord,
    ) -> Result<NotificationChannelRecord, CoreError> {
        let mut state = self.state.write().await;
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
        state.channels.insert(rec.id, rec.clone());
        Ok(rec)
    }

    async fn update_channel(
        &self,
        user_id: Uuid,
        channel_id: Uuid,
        cmd: UpdateChannelRecord,
    ) -> Result<NotificationChannelRecord, CoreError> {
        let mut state = self.state.write().await;
        let ch = state
            .channels
            .get_mut(&channel_id)
            .filter(|c| c.user_id == user_id)
            .ok_or_else(|| CoreError::NotFound("notification channel not found".to_owned()))?;
        if let Some(name) = cmd.name {
            ch.name = name;
        }
        if let Some(enabled) = cmd.enabled {
            ch.enabled = enabled;
        }
        if let Some(config_enc) = cmd.config_enc {
            ch.config_enc = config_enc;
        }
        if let Some(config_hint) = cmd.config_hint {
            ch.config_hint = config_hint;
        }
        if let Some(trigger_on_daily) = cmd.trigger_on_daily {
            ch.trigger_on_daily = trigger_on_daily;
        }
        if let Some(trigger_on_alert) = cmd.trigger_on_alert {
            ch.trigger_on_alert = trigger_on_alert;
        }
        Ok(ch.clone())
    }

    async fn delete_channel(&self, user_id: Uuid, channel_id: Uuid) -> Result<(), CoreError> {
        let mut state = self.state.write().await;
        let owned = state
            .channels
            .get(&channel_id)
            .is_some_and(|c| c.user_id == user_id);
        if !owned {
            return Err(CoreError::NotFound(
                "notification channel not found".to_owned(),
            ));
        }
        state.channels.remove(&channel_id);
        Ok(())
    }

    async fn get_schedule(&self, user_id: Uuid) -> Result<UserScheduleRecord, CoreError> {
        let mut state = self.state.write().await;
        let schedule = state
            .schedules
            .entry(user_id)
            .or_insert_with(|| default_schedule(user_id))
            .clone();
        Ok(schedule)
    }

    async fn upsert_schedule(
        &self,
        user_id: Uuid,
        cmd: UpdateScheduleCmd,
    ) -> Result<UserScheduleRecord, CoreError> {
        cmd.validate()?;
        let mut state = self.state.write().await;
        let entry = state
            .schedules
            .entry(user_id)
            .or_insert_with(|| default_schedule(user_id));
        entry.timezone = cmd.timezone.trim().to_owned();
        entry.daily_time = cmd.daily_time.trim().to_owned();
        entry.email_enabled = cmd.email_enabled;
        entry.send_when_empty = cmd.send_when_empty;
        entry.alert_scan_enabled = cmd.alert_scan_enabled;
        entry.alert_interval_minutes = cmd.alert_interval_minutes;
        Ok(entry.clone())
    }

    async fn list_due_daily_schedules(&self) -> Result<Vec<UserScheduleRecord>, CoreError> {
        let state = self.state.read().await;
        let mut schedules: Vec<UserScheduleRecord> = state.schedules.values().cloned().collect();
        schedules.sort_by_key(|s| s.user_id);
        Ok(schedules)
    }

    async fn mark_daily_run_date(
        &self,
        user_id: Uuid,
        digest_date: NaiveDate,
    ) -> Result<(), CoreError> {
        let mut state = self.state.write().await;
        let schedule = state
            .schedules
            .get_mut(&user_id)
            .ok_or_else(|| CoreError::NotFound("user schedule not found".to_owned()))?;
        schedule.last_daily_run_date = Some(digest_date);
        Ok(())
    }

    async fn list_due_alert_schedules(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<UserScheduleRecord>, CoreError> {
        let state = self.state.read().await;
        let mut schedules: Vec<UserScheduleRecord> = state
            .schedules
            .values()
            .filter(|s| {
                s.alert_scan_enabled
                    && s.last_alert_scan_at.is_none_or(|last| {
                        last + Duration::minutes(i64::from(s.alert_interval_minutes.max(1))) <= now
                    })
            })
            .cloned()
            .collect();
        schedules.sort_by_key(|s| s.user_id);
        Ok(schedules)
    }

    async fn mark_alert_scan_at(
        &self,
        user_id: Uuid,
        scanned_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let mut state = self.state.write().await;
        let schedule = state
            .schedules
            .get_mut(&user_id)
            .ok_or_else(|| CoreError::NotFound("user schedule not found".to_owned()))?;
        schedule.last_alert_scan_at = Some(scanned_at);
        Ok(())
    }

    async fn list_watchlist_rules(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<WatchlistRuleRecord>, CoreError> {
        let state = self.state.read().await;
        let mut rules: Vec<WatchlistRuleRecord> = state
            .watchlist_rules
            .values()
            .filter(|r| r.user_id == user_id)
            .cloned()
            .collect();
        rules.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(rules)
    }

    async fn create_watchlist_rule(
        &self,
        user_id: Uuid,
        cmd: UpsertWatchlistRuleCmd,
    ) -> Result<WatchlistRuleRecord, CoreError> {
        if cmd.name.trim().is_empty() || cmd.keywords.is_empty() {
            return Err(CoreError::InvalidInput(
                "watchlist rule name and keywords must not be empty".to_owned(),
            ));
        }
        let mut state = self.state.write().await;
        let rec = WatchlistRuleRecord {
            id: Uuid::new_v4(),
            user_id,
            name: cmd.name.trim().to_owned(),
            keywords: cmd.keywords,
            exclude_keywords: cmd.exclude_keywords,
            priority: cmd.priority,
            notify_immediately: cmd.notify_immediately,
            enabled: cmd.enabled,
            created_at: Utc::now(),
        };
        state.watchlist_rules.insert(rec.id, rec.clone());
        Ok(rec)
    }

    async fn update_watchlist_rule(
        &self,
        user_id: Uuid,
        rule_id: Uuid,
        cmd: UpsertWatchlistRuleCmd,
    ) -> Result<WatchlistRuleRecord, CoreError> {
        if cmd.name.trim().is_empty() || cmd.keywords.is_empty() {
            return Err(CoreError::InvalidInput(
                "watchlist rule name and keywords must not be empty".to_owned(),
            ));
        }
        let mut state = self.state.write().await;
        let rule = state
            .watchlist_rules
            .get_mut(&rule_id)
            .filter(|r| r.user_id == user_id)
            .ok_or_else(|| CoreError::NotFound("watchlist rule not found".to_owned()))?;
        rule.name = cmd.name.trim().to_owned();
        rule.keywords = cmd.keywords;
        rule.exclude_keywords = cmd.exclude_keywords;
        rule.priority = cmd.priority;
        rule.notify_immediately = cmd.notify_immediately;
        rule.enabled = cmd.enabled;
        Ok(rule.clone())
    }

    async fn delete_watchlist_rule(&self, user_id: Uuid, rule_id: Uuid) -> Result<(), CoreError> {
        let mut state = self.state.write().await;
        let owned = state
            .watchlist_rules
            .get(&rule_id)
            .is_some_and(|r| r.user_id == user_id);
        if !owned {
            return Err(CoreError::NotFound("watchlist rule not found".to_owned()));
        }
        state.watchlist_rules.remove(&rule_id);
        Ok(())
    }
}

/// Ensures `WatchlistRuleInput` stays aligned with `WatchlistRuleRecord` conversions.
#[allow(dead_code)]
fn _watchlist_to_core_check(rule: &WatchlistRuleRecord) -> WatchlistRuleInput {
    rule.to_core_input()
}

#[cfg(test)]
mod tests {
    use digest_core::constants::CRYPTO_KEY_BYTE_LEN;
    use digest_core::domain::{
        DigestSection, FetchOutcome, NotificationChannelType, RunType, SourceType,
    };

    use super::*;
    use crate::repo::dto::{encode_source_config, SaveSourceDigestCmd};

    fn test_cipher() -> Arc<EnvelopeCipher> {
        Arc::new(EnvelopeCipher::from_bytes([0x21u8; CRYPTO_KEY_BYTE_LEN]))
    }

    #[tokio::test]
    async fn shared_public_deduplicates_while_user_private_isolates_across_users() {
        let cipher = test_cipher();
        let store = Arc::new(InMemoryRepoStore::with_cipher(Arc::clone(&cipher)));

        let alice = store
            .upsert_oidc_user(UpsertUserCmd {
                oidc_sub: "alice-sub".to_owned(),
                email: Some("alice@example.com".to_owned()),
                display_name: Some("Alice".to_owned()),
                role: "member".to_owned(),
            })
            .await
            .unwrap();
        let bob = store
            .upsert_oidc_user(UpsertUserCmd {
                oidc_sub: "bob-sub".to_owned(),
                email: Some("bob@example.com".to_owned()),
                display_name: Some("Bob".to_owned()),
                role: "member".to_owned(),
            })
            .await
            .unwrap();

        // Default schedule automatically created on upsert_oidc_user
        let alice_sched = store.get_schedule(alice.id).await.unwrap();
        assert_eq!(alice_sched.timezone, DEFAULT_TIMEZONE);

        // Both subscribe to shared_public canonical source
        let enc_public =
            encode_source_config(&cipher, "https://example.com/gemini-rss.xml", None).unwrap();
        let alice_pub = store
            .create_or_subscribe(
                alice.id,
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "gemini-blog".to_owned(),
                    custom_name: "Alice Blog Name".to_owned(),
                    default_name: "Gemini Blog".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: "https://example.com/gemini-rss.xml".to_owned(),
                    config_json_enc: Some(enc_public.clone()),
                    enabled: true,
                },
            )
            .await
            .unwrap();
        let bob_pub = store
            .create_or_subscribe(
                bob.id,
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "gemini-blog".to_owned(),
                    custom_name: "Bob Blog Name".to_owned(),
                    default_name: "Gemini Blog".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: "https://example.com/gemini-rss.xml".to_owned(),
                    config_json_enc: Some(enc_public),
                    enabled: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(alice_pub.id, bob_pub.id);
        assert_ne!(alice_pub.subscription_id, bob_pub.subscription_id);

        // Both subscribe to user_private source with same canonical_key
        let alice_priv_enc = encode_source_config(
            &cipher,
            "@geminidev",
            Some(serde_json::json!({ "bearerToken": "alice-token-secret-1234" })),
        )
        .unwrap();
        let bob_priv_enc = encode_source_config(
            &cipher,
            "@geminidev",
            Some(serde_json::json!({ "bearerToken": "bob-token-secret-5678" })),
        )
        .unwrap();

        let alice_priv = store
            .create_or_subscribe(
                alice.id,
                SubscribeSourceCmd {
                    source_type: SourceType::XAccount,
                    scope: SourceScope::UserPrivate,
                    canonical_key: "x:@geminidev".to_owned(),
                    custom_name: "Alice Private X".to_owned(),
                    default_name: "@geminidev".to_owned(),
                    category: "Social".to_owned(),
                    handle_or_url: "@geminidev".to_owned(),
                    config_json_enc: Some(alice_priv_enc),
                    enabled: true,
                },
            )
            .await
            .unwrap();
        let bob_priv = store
            .create_or_subscribe(
                bob.id,
                SubscribeSourceCmd {
                    source_type: SourceType::XAccount,
                    scope: SourceScope::UserPrivate,
                    canonical_key: "x:@geminidev".to_owned(),
                    custom_name: "Bob Private X".to_owned(),
                    default_name: "@geminidev".to_owned(),
                    category: "Social".to_owned(),
                    handle_or_url: "@geminidev".to_owned(),
                    config_json_enc: Some(bob_priv_enc),
                    enabled: true,
                },
            )
            .await
            .unwrap();
        assert_ne!(alice_priv.id, bob_priv.id);
        assert!(alice_priv.has_credentials);

        // Bob cannot read Alice's private source or its items
        assert!(store
            .get_user_source(bob.id, alice_priv.id)
            .await
            .unwrap()
            .is_none());
        assert!(matches!(
            store
                .list_source_items(bob.id, alice_priv.id, PageRequest::default())
                .await,
            Err(CoreError::NotFound(_))
        ));

        // Insert items into Alice's private source and shared public source
        let priv_items = store
            .upsert_canonical_items(
                alice_priv.id,
                &[NewCanonicalItem {
                    external_id: Some("x-1".to_owned()),
                    canonical_url: "https://example.com/x/1".to_owned(),
                    content_hash: "hash-priv-1".to_owned(),
                    title: "Secret Private Alpha Gemini 2.5".to_owned(),
                    author: Some("@geminidev".to_owned()),
                    summary_raw: Some("Private summary body".to_owned()),
                    published_at: Some(Utc::now()),
                }],
            )
            .await
            .unwrap();
        assert_eq!(priv_items.len(), 1);

        // Bob searching for "Secret Private Alpha" MUST NOT see Alice's private item
        let bob_search = store
            .search_user_items(
                bob.id,
                ItemSearchQuery {
                    q: Some("Secret Private Alpha".to_owned()),
                    ..ItemSearchQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(bob_search.total, 0);

        // Alice deleting her private subscription removes the private canonical source too
        store
            .delete_user_subscription(alice.id, alice_priv.id)
            .await
            .unwrap();
        assert!(store
            .get_user_source(alice.id, alice_priv.id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn zero_subscriber_sleep_item_dedup_search_and_180_day_bookmark_retention() {
        let store = InMemoryRepoStore::new();
        let user = store
            .upsert_oidc_user(UpsertUserCmd {
                oidc_sub: "u1".to_owned(),
                email: Some("u1@example.com".to_owned()),
                display_name: None,
                role: "member".to_owned(),
            })
            .await
            .unwrap();

        let sub = store
            .create_or_subscribe(
                user.id,
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "google-ai-blog".to_owned(),
                    custom_name: "Google AI".to_owned(),
                    default_name: "Google AI".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: "https://example.com/blog/rss".to_owned(),
                    config_json_enc: None,
                    enabled: true,
                },
            )
            .await
            .unwrap();

        assert_eq!(
            store
                .list_pollable_sources(None, Utc::now(), 10)
                .await
                .unwrap()
                .len(),
            1
        );

        // Disable subscription -> zero enabled subscribers -> source sleeps
        store
            .update_user_subscription(
                user.id,
                sub.id,
                UpdateSourceCmd {
                    enabled: Some(false),
                    ..UpdateSourceCmd::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .list_pollable_sources(None, Utc::now(), 10)
                .await
                .unwrap()
                .len(),
            0
        );

        // Re-enable subscription
        store
            .update_user_subscription(
                user.id,
                sub.id,
                UpdateSourceCmd {
                    enabled: Some(true),
                    ..UpdateSourceCmd::default()
                },
            )
            .await
            .unwrap();

        // Item deduplication by canonical_url AND content_hash
        let inserted = store
            .upsert_canonical_items(
                sub.id,
                &[
                    NewCanonicalItem {
                        external_id: Some("1".to_owned()),
                        canonical_url: "https://example.com/a".to_owned(),
                        content_hash: "hash-a".to_owned(),
                        title: "Gemini 2.5 Pro Release".to_owned(),
                        author: Some("DeepMind".to_owned()),
                        summary_raw: Some(
                            "Detailed release notes with multimodal token metrics".to_owned(),
                        ),
                        published_at: Some(Utc::now()),
                    },
                    // Duplicate URL
                    NewCanonicalItem {
                        external_id: Some("1-dup-url".to_owned()),
                        canonical_url: "https://example.com/a".to_owned(),
                        content_hash: "hash-different".to_owned(),
                        title: "Duplicate URL".to_owned(),
                        author: None,
                        summary_raw: None,
                        published_at: Some(Utc::now()),
                    },
                    // Duplicate content hash
                    NewCanonicalItem {
                        external_id: Some("1-dup-hash".to_owned()),
                        canonical_url: "https://example.com/a-mirror".to_owned(),
                        content_hash: "hash-a".to_owned(),
                        title: "Duplicate Hash".to_owned(),
                        author: None,
                        summary_raw: None,
                        published_at: Some(Utc::now()),
                    },
                    NewCanonicalItem {
                        external_id: Some("2".to_owned()),
                        canonical_url: "https://example.com/b".to_owned(),
                        content_hash: "hash-b".to_owned(),
                        title: "Vertex SDK Patch".to_owned(),
                        author: Some("Cloud Team".to_owned()),
                        summary_raw: Some("Ephemeral body content only".to_owned()),
                        published_at: Some(Utc::now()),
                    },
                ],
            )
            .await
            .unwrap();
        assert_eq!(inserted.len(), 2);

        let bookmarked_item = &inserted[0];
        let unbookmarked_item = &inserted[1];

        // Match & urgent alert flow
        store
            .upsert_user_item_matches(
                user.id,
                &[NewUserItemMatch {
                    item_id: bookmarked_item.id,
                    matched_keywords: vec!["Gemini 2.5".to_owned()],
                    priority: AlertPriority::Urgent,
                    importance_score: 90,
                }],
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .list_pending_urgent_alerts(user.id)
                .await
                .unwrap()
                .len(),
            1
        );
        store
            .mark_items_alerted(user.id, &[bookmarked_item.id], Utc::now())
            .await
            .unwrap();
        assert_eq!(
            store
                .list_pending_urgent_alerts(user.id)
                .await
                .unwrap()
                .len(),
            0
        );

        // Bookmark item 1; leave item 2 unbookmarked
        store
            .upsert_bookmark(user.id, bookmarked_item.id, Some("Keep forever".to_owned()))
            .await
            .unwrap();

        // Filter search by keyword + bookmarked
        let kw_res = store
            .search_user_items(
                user.id,
                ItemSearchQuery {
                    q: Some("multimodal".to_owned()),
                    keyword: Some("gemini 2.5".to_owned()),
                    bookmarked: Some(true),
                    ..ItemSearchQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(kw_res.total, 1);
        assert_eq!(kw_res.items[0].id, bookmarked_item.id);

        // Prune items older than future cutoff (simulates >180d)
        let pruned_count = store
            .prune_unbookmarked_bodies(Utc::now() + Duration::days(1))
            .await
            .unwrap();
        assert_eq!(pruned_count, 1);

        // Bookmarked item retains body; unbookmarked item has body_pruned=true and summary=None
        let all_items = store
            .list_source_items(user.id, sub.id, PageRequest::default())
            .await
            .unwrap();
        let kept = all_items
            .items
            .iter()
            .find(|i| i.id == bookmarked_item.id)
            .unwrap();
        assert!(!kept.body_pruned);
        assert!(kept.summary.is_some());

        let pruned = all_items
            .items
            .iter()
            .find(|i| i.id == unbookmarked_item.id)
            .unwrap();
        assert!(pruned.body_pruned);
        assert!(pruned.summary.is_none());

        // Searching pruned body text returns 0, while title remains searchable
        let pruned_body_search = store
            .search_user_items(
                user.id,
                ItemSearchQuery {
                    q: Some("Ephemeral".to_owned()),
                    ..ItemSearchQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(pruned_body_search.total, 0);

        let pruned_title_search = store
            .search_user_items(
                user.id,
                ItemSearchQuery {
                    q: Some("Vertex SDK".to_owned()),
                    ..ItemSearchQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(pruned_title_search.total, 1);
    }

    #[tokio::test]
    async fn digests_runs_deliveries_and_settings_crud_work() {
        let store = InMemoryRepoStore::new();
        let user = store
            .upsert_oidc_user(UpsertUserCmd {
                oidc_sub: "u-digest".to_owned(),
                email: Some("digest@example.com".to_owned()),
                display_name: Some("Digest User".to_owned()),
                role: "member".to_owned(),
            })
            .await
            .unwrap();

        let sub = store
            .create_or_subscribe(
                user.id,
                SubscribeSourceCmd {
                    source_type: SourceType::GithubRelease,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "google-gemini/generative-ai-js".to_owned(),
                    custom_name: "Gemini JS SDK".to_owned(),
                    default_name: "Gemini JS SDK".to_owned(),
                    category: "Releases".to_owned(),
                    handle_or_url: "google-gemini/generative-ai-js".to_owned(),
                    config_json_enc: None,
                    enabled: true,
                },
            )
            .await
            .unwrap();

        let today = Utc::now().date_naive();
        let saved = store
            .save_daily_and_source_digests(SaveDigestCmd {
                user_id: user.id,
                digest_date: today,
                input_fingerprint: "fp-123".to_owned(),
                title: "Daily Gemini Digest".to_owned(),
                overview_markdown: "## Overview".to_owned(),
                overview_html: "<h2>Overview</h2>".to_owned(),
                sections: vec![DigestSection {
                    category: "Releases".to_owned(),
                    source_id: Some(sub.id),
                    source_name: "Gemini JS SDK".to_owned(),
                    summary_markdown: "v1.0".to_owned(),
                    key_points: vec!["Streaming support".to_owned()],
                    items: Vec::new(),
                }],
                item_count: 1,
                highlighted_count: 1,
                prompt_tokens: 120,
                completion_tokens: 45,
                model_used: "deepseek-chat".to_owned(),
                degraded_fallback: false,
                source_digests: vec![SaveSourceDigestCmd {
                    canonical_source_id: sub.id,
                    digest_date: today,
                    input_fingerprint: "fp-src-1".to_owned(),
                    summary_markdown: "SDK update".to_owned(),
                    key_points: vec!["Streaming support".to_owned()],
                    item_count: 1,
                }],
            })
            .await
            .unwrap();

        assert!(store
            .find_daily_digest_by_fingerprint(user.id, today, "fp-123")
            .await
            .unwrap()
            .is_some());
        assert!(store
            .find_daily_digest_by_fingerprint(user.id, today, "wrong-fp")
            .await
            .unwrap()
            .is_none());

        let src_digests = store
            .list_source_daily_digests(user.id, sub.id, PageRequest::default())
            .await
            .unwrap();
        assert_eq!(src_digests.total, 1);
        assert_eq!(src_digests.items[0].source_name, "Gemini JS SDK");

        let run = store
            .create_run(CreateRunCmd {
                user_id: user.id,
                run_type: RunType::ManualDaily,
                started_at: Utc::now(),
            })
            .await
            .unwrap();
        store
            .finish_run(
                run.id,
                FinishRunCmd {
                    status: RunStatus::Succeeded,
                    finished_at: Utc::now(),
                    items_fetched: 5,
                    items_new: 1,
                    prompt_tokens: 120,
                    completion_tokens: 45,
                    error_summary: None,
                },
            )
            .await
            .unwrap();

        let channel = store
            .create_channel(
                user.id,
                CreateChannelRecord {
                    channel_type: NotificationChannelType::Webhook,
                    name: "My Webhook".to_owned(),
                    enabled: true,
                    config_enc: "v1:encrypted".to_owned(),
                    config_hint: "https://example.com/****".to_owned(),
                    trigger_on_daily: true,
                    trigger_on_alert: true,
                },
            )
            .await
            .unwrap();

        let delivery = store
            .record_delivery(CreateDeliveryLogCmd {
                run_id: run.id,
                user_id: user.id,
                digest_id: Some(saved.id),
                channel_id: Some(channel.id),
                channel_type: NotificationChannelType::Webhook,
                status: DeliveryStatus::Failed,
                error_message: Some("timeout".to_owned()),
                sent_at: Utc::now(),
            })
            .await
            .unwrap();

        store
            .update_delivery_status(user.id, delivery.id, DeliveryStatus::Sent, None, Utc::now())
            .await
            .unwrap();

        store
            .record_fetch_outcome(
                sub.id,
                SourceFetchStateUpdate {
                    outcome: FetchOutcome::Updated,
                    etag: Some("\"etag-1\"".to_owned()),
                    last_modified: None,
                    fetched_at: Utc::now(),
                    next_poll_at: None,
                    last_error: None,
                    consecutive_failures: 0,
                },
            )
            .await
            .unwrap();

        let stats = store.get_run_stats(user.id).await.unwrap();
        assert_eq!(stats.total_runs, 1);
        assert_eq!(stats.succeeded_runs, 1);
        assert_eq!(stats.total_prompt_tokens, 120);
        assert_eq!(stats.deliveries_sent, 1);
        assert_eq!(stats.deliveries_failed, 0);
        assert_eq!(stats.source_health.len(), 1);
        assert_eq!(
            stats.source_health[0].last_status,
            Some(FetchOutcome::Updated)
        );

        // Watchlist CRUD
        let rule = store
            .create_watchlist_rule(
                user.id,
                UpsertWatchlistRuleCmd {
                    name: "Gemini".to_owned(),
                    keywords: vec!["gemini".to_owned()],
                    exclude_keywords: vec![],
                    priority: AlertPriority::Urgent,
                    notify_immediately: true,
                    enabled: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(store.list_watchlist_rules(user.id).await.unwrap().len(), 1);
        store.delete_watchlist_rule(user.id, rule.id).await.unwrap();
        assert_eq!(store.list_watchlist_rules(user.id).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn onboarding_flags_schedule_intervals_and_shared_config_guard_work() {
        let cipher = test_cipher();
        let store = InMemoryRepoStore::with_cipher(Arc::clone(&cipher));

        let user = store
            .upsert_oidc_user(UpsertUserCmd {
                oidc_sub: "onboard-user".to_owned(),
                email: Some("onboard@example.com".to_owned()),
                display_name: None,
                role: "member".to_owned(),
            })
            .await
            .unwrap();

        let flags0 = store.get_onboarding_flags(user.id).await.unwrap();
        assert!(!flags0.has_ai_config);
        assert!(!flags0.has_sources);
        assert!(!flags0.has_channels);
        assert!(flags0.has_schedule);

        store
            .upsert_ai_config(
                user.id,
                UpsertAiConfigRecord {
                    provider_label: "DeepSeek".to_owned(),
                    base_url: "https://api.deepseek.com".to_owned(),
                    api_key_enc: Some("v1:enc".to_owned()),
                    api_key_hint: Some("test****1234".to_owned()),
                    model: "deepseek-chat".to_owned(),
                    system_prompt: "prompt".to_owned(),
                    temperature: 0.2,
                    max_tokens: 2048,
                },
            )
            .await
            .unwrap();

        let pub_sub = store
            .create_or_subscribe(
                user.id,
                SubscribeSourceCmd {
                    source_type: SourceType::Rss,
                    scope: SourceScope::SharedPublic,
                    canonical_key: "google-ai-blog".to_owned(),
                    custom_name: "Blog".to_owned(),
                    default_name: "Google AI Blog".to_owned(),
                    category: "Official".to_owned(),
                    handle_or_url: "https://example.com/blog.xml".to_owned(),
                    config_json_enc: None,
                    enabled: true,
                },
            )
            .await
            .unwrap();

        // Lookup works by both canonical_source_id and subscription_id
        assert!(store
            .get_user_source(user.id, pub_sub.id)
            .await
            .unwrap()
            .is_some());
        assert!(store
            .get_user_source(user.id, pub_sub.subscription_id)
            .await
            .unwrap()
            .is_some());

        // Attempting to overwrite shared_public canonical source config is forbidden
        let err = store
            .update_user_subscription(
                user.id,
                pub_sub.id,
                UpdateSourceCmd {
                    config_json_enc: Some("v1:hijack".to_owned()),
                    ..UpdateSourceCmd::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Forbidden(_)));

        // Alert schedule interval gating
        let now = Utc::now();
        assert_eq!(store.list_due_alert_schedules(now).await.unwrap().len(), 1);
        store.mark_alert_scan_at(user.id, now).await.unwrap();
        assert_eq!(store.list_due_alert_schedules(now).await.unwrap().len(), 0);
        assert_eq!(
            store
                .list_due_alert_schedules(now + Duration::minutes(61))
                .await
                .unwrap()
                .len(),
            1
        );

        let flags1 = store.get_onboarding_flags(user.id).await.unwrap();
        assert!(flags1.has_ai_config);
        assert!(flags1.has_sources);
        assert!(!flags1.has_channels);
        assert!(flags1.has_schedule);
    }
}
