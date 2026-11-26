//! Repository abstractions (DIP) and implementations (`postgres`, `in_memory`).

pub mod dto;
pub mod in_memory;
pub mod postgres;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use digest_core::domain::{DeliveryStatus, SourceScope};
use digest_core::error::CoreError;
use uuid::Uuid;

pub use dto::*;

/// User identity and onboarding status persistence.
#[async_trait]
pub trait UserRepo: Send + Sync {
    async fn upsert_oidc_user(&self, cmd: UpsertUserCmd) -> Result<UserRecord, CoreError>;
    async fn get_user_by_id(&self, user_id: Uuid) -> Result<Option<UserRecord>, CoreError>;
    async fn update_user_locale(
        &self,
        user_id: Uuid,
        preferred_locale: &str,
    ) -> Result<UserRecord, CoreError>;
    async fn get_onboarding_flags(&self, user_id: Uuid) -> Result<OnboardingFlags, CoreError>;
}

/// Shared canonical source pool, per-user subscriptions, canonical items, matches, and bookmarks.
#[async_trait]
pub trait SourceRepo: Send + Sync {
    async fn create_or_subscribe(
        &self,
        user_id: Uuid,
        cmd: SubscribeSourceCmd,
    ) -> Result<UserSourceView, CoreError>;

    async fn list_user_sources(&self, user_id: Uuid) -> Result<Vec<UserSourceView>, CoreError>;

    async fn get_user_source(
        &self,
        user_id: Uuid,
        id_or_sub: Uuid,
    ) -> Result<Option<UserSourceRecord>, CoreError>;

    async fn update_user_subscription(
        &self,
        user_id: Uuid,
        id_or_sub: Uuid,
        cmd: UpdateSourceCmd,
    ) -> Result<UserSourceView, CoreError>;

    async fn delete_user_subscription(
        &self,
        user_id: Uuid,
        id_or_sub: Uuid,
    ) -> Result<(), CoreError>;

    async fn list_pollable_sources(
        &self,
        scope: Option<SourceScope>,
        due_before: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<CanonicalSourceRecord>, CoreError>;

    async fn record_fetch_outcome(
        &self,
        source_id: Uuid,
        update: SourceFetchStateUpdate,
    ) -> Result<(), CoreError>;

    async fn upsert_canonical_items(
        &self,
        source_id: Uuid,
        items: &[NewCanonicalItem],
    ) -> Result<Vec<CanonicalItemRecord>, CoreError>;

    async fn list_recent_source_items(
        &self,
        source_id: Uuid,
        since: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<CanonicalItemRecord>, CoreError>;

    async fn list_source_items(
        &self,
        user_id: Uuid,
        source_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<ItemView>, CoreError>;

    async fn search_user_items(
        &self,
        user_id: Uuid,
        query: ItemSearchQuery,
    ) -> Result<PagedResult<ItemView>, CoreError>;

    async fn upsert_user_item_matches(
        &self,
        user_id: Uuid,
        matches: &[NewUserItemMatch],
    ) -> Result<(), CoreError>;

    async fn list_pending_urgent_alerts(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<PendingAlertItem>, CoreError>;

    async fn mark_items_alerted(
        &self,
        user_id: Uuid,
        item_ids: &[Uuid],
        alerted_at: DateTime<Utc>,
    ) -> Result<(), CoreError>;

    async fn prune_unbookmarked_bodies(&self, cutoff: DateTime<Utc>) -> Result<u64, CoreError>;

    async fn upsert_bookmark(
        &self,
        user_id: Uuid,
        item_id: Uuid,
        note: Option<String>,
    ) -> Result<BookmarkView, CoreError>;

    async fn delete_bookmark(&self, user_id: Uuid, item_id: Uuid) -> Result<(), CoreError>;
}

/// Daily digests and per-source daily digest summaries.
#[async_trait]
pub trait DigestRepo: Send + Sync {
    async fn list_daily_digests(
        &self,
        user_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<DailyDigestSummary>, CoreError>;

    async fn get_daily_digest_by_date(
        &self,
        user_id: Uuid,
        date: NaiveDate,
    ) -> Result<Option<DailyDigestDetail>, CoreError>;

    async fn find_daily_digest_by_fingerprint(
        &self,
        user_id: Uuid,
        date: NaiveDate,
        fingerprint: &str,
    ) -> Result<Option<DailyDigestDetail>, CoreError>;

    async fn save_daily_and_source_digests(
        &self,
        cmd: SaveDigestCmd,
    ) -> Result<DailyDigestDetail, CoreError>;

    async fn list_source_daily_digests(
        &self,
        user_id: Uuid,
        source_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<SourceDailyDigestView>, CoreError>;
}

/// Execution run history, notification delivery logs, and aggregated metrics.
#[async_trait]
pub trait RunRepo: Send + Sync {
    async fn create_run(&self, cmd: CreateRunCmd) -> Result<DigestRunRecord, CoreError>;

    async fn finish_run(&self, run_id: Uuid, update: FinishRunCmd) -> Result<(), CoreError>;

    async fn list_runs_with_deliveries(
        &self,
        user_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<DigestRunView>, CoreError>;

    async fn record_delivery(
        &self,
        cmd: CreateDeliveryLogCmd,
    ) -> Result<DeliveryLogRecord, CoreError>;

    async fn get_delivery_by_id(
        &self,
        user_id: Uuid,
        delivery_id: Uuid,
    ) -> Result<Option<DeliveryLogRecord>, CoreError>;

    async fn update_delivery_status(
        &self,
        user_id: Uuid,
        delivery_id: Uuid,
        status: DeliveryStatus,
        error_message: Option<String>,
        sent_at: DateTime<Utc>,
    ) -> Result<DeliveryLogRecord, CoreError>;

    async fn get_run_stats(&self, user_id: Uuid) -> Result<RunStatsView, CoreError>;
}

/// User-scoped settings: AI config, notification channels, schedules, and watchlist rules.
#[async_trait]
pub trait SettingsRepo: Send + Sync {
    async fn get_ai_config(&self, user_id: Uuid) -> Result<Option<UserAiConfigRecord>, CoreError>;

    async fn upsert_ai_config(
        &self,
        user_id: Uuid,
        cmd: UpsertAiConfigRecord,
    ) -> Result<UserAiConfigRecord, CoreError>;

    async fn list_channels(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<NotificationChannelRecord>, CoreError>;

    async fn get_channel(
        &self,
        user_id: Uuid,
        channel_id: Uuid,
    ) -> Result<Option<NotificationChannelRecord>, CoreError>;

    async fn create_channel(
        &self,
        user_id: Uuid,
        cmd: CreateChannelRecord,
    ) -> Result<NotificationChannelRecord, CoreError>;

    async fn update_channel(
        &self,
        user_id: Uuid,
        channel_id: Uuid,
        cmd: UpdateChannelRecord,
    ) -> Result<NotificationChannelRecord, CoreError>;

    async fn delete_channel(&self, user_id: Uuid, channel_id: Uuid) -> Result<(), CoreError>;

    async fn get_schedule(&self, user_id: Uuid) -> Result<UserScheduleRecord, CoreError>;

    async fn upsert_schedule(
        &self,
        user_id: Uuid,
        cmd: UpdateScheduleCmd,
    ) -> Result<UserScheduleRecord, CoreError>;

    async fn list_due_daily_schedules(&self) -> Result<Vec<UserScheduleRecord>, CoreError>;

    async fn mark_daily_run_date(
        &self,
        user_id: Uuid,
        digest_date: NaiveDate,
    ) -> Result<(), CoreError>;

    async fn list_due_alert_schedules(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<UserScheduleRecord>, CoreError>;

    async fn mark_alert_scan_at(
        &self,
        user_id: Uuid,
        scanned_at: DateTime<Utc>,
    ) -> Result<(), CoreError>;

    async fn list_watchlist_rules(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<WatchlistRuleRecord>, CoreError>;

    async fn create_watchlist_rule(
        &self,
        user_id: Uuid,
        cmd: UpsertWatchlistRuleCmd,
    ) -> Result<WatchlistRuleRecord, CoreError>;

    async fn update_watchlist_rule(
        &self,
        user_id: Uuid,
        rule_id: Uuid,
        cmd: UpsertWatchlistRuleCmd,
    ) -> Result<WatchlistRuleRecord, CoreError>;

    async fn delete_watchlist_rule(&self, user_id: Uuid, rule_id: Uuid) -> Result<(), CoreError>;

    async fn get_platform_settings(&self) -> Result<Option<PlatformSettingsRecord>, CoreError> {
        Ok(None)
    }

    async fn upsert_platform_settings(
        &self,
        _cmd: UpsertPlatformSettingsRecord,
    ) -> Result<PlatformSettingsRecord, CoreError> {
        Err(CoreError::InvalidConfig(
            "platform settings persistence is not implemented on this store".to_owned(),
        ))
    }
}

/// Bundled trait-object repositories injected into `AppState` and scheduler services.
#[derive(Clone)]
pub struct Repositories {
    pub users: Arc<dyn UserRepo>,
    pub sources: Arc<dyn SourceRepo>,
    pub digests: Arc<dyn DigestRepo>,
    pub runs: Arc<dyn RunRepo>,
    pub settings: Arc<dyn SettingsRepo>,
}
