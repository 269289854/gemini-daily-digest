//! PostgreSQL repository implementation backed by `sqlx::PgPool`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use digest_core::constants::{
    DEFAULT_ALERT_INTERVAL_MINUTES, DEFAULT_DAILY_TIME, DEFAULT_TIMEZONE,
    SECRET_MASK_CONFIGURED_PLACEHOLDER,
};
use digest_core::crypto::EnvelopeCipher;
use digest_core::domain::{
    AlertPriority, DeliveryStatus, DigestSection, FetchOutcome, NotificationChannelType, RunStatus,
    RunType, SourceScope, SourceType,
};
use digest_core::error::CoreError;
use digest_core::presets::find_preset_by_key;
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::types::Json;
use sqlx::{PgPool, Row};
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

const MIGRATION_0001_SQL: &str = include_str!("../../migrations/0001_init.sql");

#[derive(Debug)]
struct EmbeddedMigrations;

impl sqlx::migrate::MigrationSource<'static> for EmbeddedMigrations {
    fn resolve(
        self,
    ) -> futures::future::BoxFuture<
        'static,
        Result<Vec<sqlx::migrate::Migration>, sqlx::error::BoxDynError>,
    > {
        Box::pin(async move {
            Ok(vec![sqlx::migrate::Migration::new(
                1,
                std::borrow::Cow::Borrowed("init"),
                sqlx::migrate::MigrationType::Simple,
                std::borrow::Cow::Borrowed(MIGRATION_0001_SQL),
                false,
            )])
        })
    }
}

/// Creates a bounded `sqlx::PgPool` using `PG_MAX_CONNECTIONS`.
pub async fn connect_pg_pool(
    database_url: &str,
    max_connections: u32,
) -> Result<PgPool, CoreError> {
    PgPoolOptions::new()
        .max_connections(max_connections.max(1))
        .connect(database_url)
        .await
        .map_err(|err| CoreError::Database(format!("failed to connect to PostgreSQL: {err}")))
}

/// Executes embedded SQLx PostgreSQL migrations (`./migrations`).
pub async fn run_migrations(pool: &PgPool) -> Result<(), CoreError> {
    let migrator = sqlx::migrate::Migrator::new(EmbeddedMigrations)
        .await
        .map_err(|err| {
            CoreError::Database(format!("failed to load PostgreSQL migrations: {err}"))
        })?;
    migrator
        .run(pool)
        .await
        .map_err(|err| CoreError::Database(format!("failed to run PostgreSQL migrations: {err}")))
}

/// PostgreSQL-backed implementation of all 5 repository traits (`sqlx::PgPool`).
#[derive(Clone)]
pub struct PostgresRepoStore {
    pool: PgPool,
    cipher: Option<Arc<EnvelopeCipher>>,
}

/// Type alias matching `PostgresRepositories::new(pool)` conventions.
pub type PostgresRepositories = PostgresRepoStore;

impl PostgresRepoStore {
    /// Creates a new `PostgresRepoStore` sharing the given `PgPool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool, cipher: None }
    }

    /// Creates a new `PostgresRepoStore` with an `EnvelopeCipher` for decrypting `config_json_enc`.
    #[must_use]
    pub fn with_cipher(pool: PgPool, cipher: Arc<EnvelopeCipher>) -> Self {
        Self {
            pool,
            cipher: Some(cipher),
        }
    }

    /// Returns a reference to the underlying `PgPool`.
    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Wraps this store into a `Repositories` bundle.
    #[must_use]
    pub fn into_repositories(self) -> Repositories {
        let store = Arc::new(self);
        Repositories {
            users: Arc::clone(&store) as Arc<dyn UserRepo>,
            sources: Arc::clone(&store) as Arc<dyn SourceRepo>,
            digests: Arc::clone(&store) as Arc<dyn DigestRepo>,
            runs: Arc::clone(&store) as Arc<dyn RunRepo>,
            settings: store as Arc<dyn SettingsRepo>,
        }
    }

    fn record_to_source_view(&self, rec: &UserSourceRecord) -> Result<UserSourceView, CoreError> {
        if let Some(ref cipher) = self.cipher {
            return rec.to_view(cipher);
        }

        let handle_or_url = find_preset_by_key(&rec.canonical_key)
            .map(|p| p.handle_or_url)
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

/// Builds a `Repositories` bundle backed by `PostgresRepoStore`.
#[must_use]
pub fn repositories(pool: PgPool) -> Repositories {
    PostgresRepoStore::new(pool).into_repositories()
}

/// Builds a `Repositories` bundle backed by `PostgresRepoStore` with an `EnvelopeCipher`.
#[must_use]
pub fn repositories_with_cipher(pool: PgPool, cipher: Arc<EnvelopeCipher>) -> Repositories {
    PostgresRepoStore::with_cipher(pool, cipher).into_repositories()
}

fn map_pg_err(err: sqlx::Error) -> CoreError {
    match &err {
        sqlx::Error::RowNotFound => CoreError::NotFound("requested record not found".to_owned()),
        sqlx::Error::Database(db_err) => match db_err.code().as_deref() {
            Some("23505") => CoreError::Conflict("resource already exists".to_owned()),
            Some("23503") => CoreError::NotFound("referenced parent record not found".to_owned()),
            Some("23514") | Some("22P02") => {
                CoreError::InvalidInput("database constraint or format violation".to_owned())
            }
            _ => CoreError::Database(format!("database operation failed: {err}")),
        },
        _ => CoreError::Database(format!("database operation failed: {err}")),
    }
}

fn col_err(col: &'static str, err: sqlx::Error) -> CoreError {
    CoreError::Database(format!("failed to decode column `{col}`: {err}"))
}

fn parse_user_row(row: &PgRow) -> Result<UserRecord, CoreError> {
    Ok(UserRecord {
        id: row.try_get("id").map_err(|e| col_err("id", e))?,
        oidc_sub: row
            .try_get("oidc_sub")
            .map_err(|e| col_err("oidc_sub", e))?,
        email: row.try_get("email").map_err(|e| col_err("email", e))?,
        display_name: row
            .try_get("display_name")
            .map_err(|e| col_err("display_name", e))?,
        role: row.try_get("role").map_err(|e| col_err("role", e))?,
        created_at: row
            .try_get("created_at")
            .map_err(|e| col_err("created_at", e))?,
        last_login_at: row
            .try_get("last_login_at")
            .map_err(|e| col_err("last_login_at", e))?,
    })
}

fn parse_canonical_source_row(row: &PgRow) -> Result<CanonicalSourceRecord, CoreError> {
    let source_type_raw: String = row
        .try_get("source_type")
        .map_err(|e| col_err("source_type", e))?;
    let scope_raw: String = row.try_get("scope").map_err(|e| col_err("scope", e))?;
    let status_raw: Option<String> = row
        .try_get("last_status")
        .map_err(|e| col_err("last_status", e))?;
    Ok(CanonicalSourceRecord {
        id: row.try_get("id").map_err(|e| col_err("id", e))?,
        source_type: source_type_raw.parse::<SourceType>()?,
        canonical_key: row
            .try_get("canonical_key")
            .map_err(|e| col_err("canonical_key", e))?,
        scope: scope_raw.parse::<SourceScope>()?,
        scope_owner_id: row
            .try_get("scope_owner_id")
            .map_err(|e| col_err("scope_owner_id", e))?,
        default_name: row
            .try_get("default_name")
            .map_err(|e| col_err("default_name", e))?,
        config_json_enc: row
            .try_get("config_json_enc")
            .map_err(|e| col_err("config_json_enc", e))?,
        etag: row.try_get("etag").map_err(|e| col_err("etag", e))?,
        last_modified: row
            .try_get("last_modified")
            .map_err(|e| col_err("last_modified", e))?,
        last_fetched_at: row
            .try_get("last_fetched_at")
            .map_err(|e| col_err("last_fetched_at", e))?,
        next_poll_at: row
            .try_get("next_poll_at")
            .map_err(|e| col_err("next_poll_at", e))?,
        last_status: status_raw
            .as_deref()
            .map(str::parse::<FetchOutcome>)
            .transpose()?,
        last_error: row
            .try_get("last_error")
            .map_err(|e| col_err("last_error", e))?,
        consecutive_failures: row
            .try_get("consecutive_failures")
            .map_err(|e| col_err("consecutive_failures", e))?,
        created_at: row
            .try_get("created_at")
            .map_err(|e| col_err("created_at", e))?,
    })
}

fn parse_user_source_row(row: &PgRow) -> Result<UserSourceRecord, CoreError> {
    let source_type_raw: String = row
        .try_get("source_type")
        .map_err(|e| col_err("source_type", e))?;
    let scope_raw: String = row.try_get("scope").map_err(|e| col_err("scope", e))?;
    let status_raw: Option<String> = row
        .try_get("last_status")
        .map_err(|e| col_err("last_status", e))?;
    Ok(UserSourceRecord {
        subscription_id: row
            .try_get("subscription_id")
            .map_err(|e| col_err("subscription_id", e))?,
        canonical_source_id: row
            .try_get("canonical_source_id")
            .map_err(|e| col_err("canonical_source_id", e))?,
        user_id: row.try_get("user_id").map_err(|e| col_err("user_id", e))?,
        source_type: source_type_raw.parse::<SourceType>()?,
        scope: scope_raw.parse::<SourceScope>()?,
        scope_owner_id: row
            .try_get("scope_owner_id")
            .map_err(|e| col_err("scope_owner_id", e))?,
        canonical_key: row
            .try_get("canonical_key")
            .map_err(|e| col_err("canonical_key", e))?,
        default_name: row
            .try_get("default_name")
            .map_err(|e| col_err("default_name", e))?,
        custom_name: row
            .try_get("custom_name")
            .map_err(|e| col_err("custom_name", e))?,
        category: row
            .try_get("category")
            .map_err(|e| col_err("category", e))?,
        enabled: row.try_get("enabled").map_err(|e| col_err("enabled", e))?,
        config_json_enc: row
            .try_get("config_json_enc")
            .map_err(|e| col_err("config_json_enc", e))?,
        etag: row.try_get("etag").map_err(|e| col_err("etag", e))?,
        last_modified: row
            .try_get("last_modified")
            .map_err(|e| col_err("last_modified", e))?,
        last_fetched_at: row
            .try_get("last_fetched_at")
            .map_err(|e| col_err("last_fetched_at", e))?,
        next_poll_at: row
            .try_get("next_poll_at")
            .map_err(|e| col_err("next_poll_at", e))?,
        last_status: status_raw
            .as_deref()
            .map(str::parse::<FetchOutcome>)
            .transpose()?,
        last_error: row
            .try_get("last_error")
            .map_err(|e| col_err("last_error", e))?,
        consecutive_failures: row
            .try_get("consecutive_failures")
            .map_err(|e| col_err("consecutive_failures", e))?,
        created_at: row
            .try_get("created_at")
            .map_err(|e| col_err("created_at", e))?,
    })
}

fn parse_canonical_item_row(row: &PgRow) -> Result<CanonicalItemRecord, CoreError> {
    Ok(CanonicalItemRecord {
        id: row.try_get("id").map_err(|e| col_err("id", e))?,
        canonical_source_id: row
            .try_get("canonical_source_id")
            .map_err(|e| col_err("canonical_source_id", e))?,
        external_id: row
            .try_get("external_id")
            .map_err(|e| col_err("external_id", e))?,
        canonical_url: row
            .try_get("canonical_url")
            .map_err(|e| col_err("canonical_url", e))?,
        content_hash: row
            .try_get("content_hash")
            .map_err(|e| col_err("content_hash", e))?,
        title: row.try_get("title").map_err(|e| col_err("title", e))?,
        author: row.try_get("author").map_err(|e| col_err("author", e))?,
        summary_raw: row
            .try_get("summary_raw")
            .map_err(|e| col_err("summary_raw", e))?,
        body_pruned: row
            .try_get("body_pruned")
            .map_err(|e| col_err("body_pruned", e))?,
        published_at: row
            .try_get("published_at")
            .map_err(|e| col_err("published_at", e))?,
        fetched_at: row
            .try_get("fetched_at")
            .map_err(|e| col_err("fetched_at", e))?,
    })
}

fn parse_item_view_row(row: &PgRow) -> Result<ItemView, CoreError> {
    let priority_raw: String = row
        .try_get("priority")
        .map_err(|e| col_err("priority", e))?;
    Ok(ItemView {
        id: row.try_get("id").map_err(|e| col_err("id", e))?,
        source_id: row
            .try_get("source_id")
            .map_err(|e| col_err("source_id", e))?,
        source_name: row
            .try_get("source_name")
            .map_err(|e| col_err("source_name", e))?,
        category: row
            .try_get("category")
            .map_err(|e| col_err("category", e))?,
        external_id: row
            .try_get("external_id")
            .map_err(|e| col_err("external_id", e))?,
        canonical_url: row
            .try_get("canonical_url")
            .map_err(|e| col_err("canonical_url", e))?,
        title: row.try_get("title").map_err(|e| col_err("title", e))?,
        author: row.try_get("author").map_err(|e| col_err("author", e))?,
        summary: row.try_get("summary").map_err(|e| col_err("summary", e))?,
        body_pruned: row
            .try_get("body_pruned")
            .map_err(|e| col_err("body_pruned", e))?,
        published_at: row
            .try_get("published_at")
            .map_err(|e| col_err("published_at", e))?,
        fetched_at: row
            .try_get("fetched_at")
            .map_err(|e| col_err("fetched_at", e))?,
        matched_keywords: row
            .try_get("matched_keywords")
            .map_err(|e| col_err("matched_keywords", e))?,
        priority: priority_raw.parse::<AlertPriority>()?,
        importance_score: row
            .try_get("importance_score")
            .map_err(|e| col_err("importance_score", e))?,
        bookmarked: row
            .try_get("bookmarked")
            .map_err(|e| col_err("bookmarked", e))?,
        bookmark_note: row
            .try_get("bookmark_note")
            .map_err(|e| col_err("bookmark_note", e))?,
    })
}

fn parse_daily_digest_detail_row(row: &PgRow) -> Result<DailyDigestDetail, CoreError> {
    let sections: Json<Vec<DigestSection>> = row
        .try_get("sections_json")
        .map_err(|e| col_err("sections_json", e))?;
    Ok(DailyDigestDetail {
        id: row.try_get("id").map_err(|e| col_err("id", e))?,
        digest_date: row
            .try_get("digest_date")
            .map_err(|e| col_err("digest_date", e))?,
        input_fingerprint: row
            .try_get("input_fingerprint")
            .map_err(|e| col_err("input_fingerprint", e))?,
        title: row.try_get("title").map_err(|e| col_err("title", e))?,
        overview_markdown: row
            .try_get("overview_markdown")
            .map_err(|e| col_err("overview_markdown", e))?,
        overview_html: row
            .try_get("overview_html")
            .map_err(|e| col_err("overview_html", e))?,
        sections: sections.0,
        item_count: row
            .try_get("item_count")
            .map_err(|e| col_err("item_count", e))?,
        highlighted_count: row
            .try_get("highlighted_count")
            .map_err(|e| col_err("highlighted_count", e))?,
        prompt_tokens: row
            .try_get("prompt_tokens")
            .map_err(|e| col_err("prompt_tokens", e))?,
        completion_tokens: row
            .try_get("completion_tokens")
            .map_err(|e| col_err("completion_tokens", e))?,
        model_used: row
            .try_get("model_used")
            .map_err(|e| col_err("model_used", e))?,
        degraded_fallback: row
            .try_get("degraded_fallback")
            .map_err(|e| col_err("degraded_fallback", e))?,
        created_at: row
            .try_get("created_at")
            .map_err(|e| col_err("created_at", e))?,
    })
}

fn parse_digest_run_row(row: &PgRow) -> Result<DigestRunRecord, CoreError> {
    let run_type_raw: String = row
        .try_get("run_type")
        .map_err(|e| col_err("run_type", e))?;
    let status_raw: String = row.try_get("status").map_err(|e| col_err("status", e))?;
    Ok(DigestRunRecord {
        id: row.try_get("id").map_err(|e| col_err("id", e))?,
        user_id: row.try_get("user_id").map_err(|e| col_err("user_id", e))?,
        run_type: run_type_raw.parse::<RunType>()?,
        status: status_raw.parse::<RunStatus>()?,
        started_at: row
            .try_get("started_at")
            .map_err(|e| col_err("started_at", e))?,
        finished_at: row
            .try_get("finished_at")
            .map_err(|e| col_err("finished_at", e))?,
        items_fetched: row
            .try_get("items_fetched")
            .map_err(|e| col_err("items_fetched", e))?,
        items_new: row
            .try_get("items_new")
            .map_err(|e| col_err("items_new", e))?,
        prompt_tokens: row
            .try_get("prompt_tokens")
            .map_err(|e| col_err("prompt_tokens", e))?,
        completion_tokens: row
            .try_get("completion_tokens")
            .map_err(|e| col_err("completion_tokens", e))?,
        error_summary: row
            .try_get("error_summary")
            .map_err(|e| col_err("error_summary", e))?,
    })
}

fn parse_delivery_log_row(row: &PgRow) -> Result<DeliveryLogRecord, CoreError> {
    let channel_type_raw: String = row
        .try_get("channel_type")
        .map_err(|e| col_err("channel_type", e))?;
    let status_raw: String = row.try_get("status").map_err(|e| col_err("status", e))?;
    Ok(DeliveryLogRecord {
        id: row.try_get("id").map_err(|e| col_err("id", e))?,
        run_id: row.try_get("run_id").map_err(|e| col_err("run_id", e))?,
        user_id: row.try_get("user_id").map_err(|e| col_err("user_id", e))?,
        digest_id: row
            .try_get("digest_id")
            .map_err(|e| col_err("digest_id", e))?,
        channel_id: row
            .try_get("channel_id")
            .map_err(|e| col_err("channel_id", e))?,
        channel_type: channel_type_raw.parse::<NotificationChannelType>()?,
        status: status_raw.parse::<DeliveryStatus>()?,
        error_message: row
            .try_get("error_message")
            .map_err(|e| col_err("error_message", e))?,
        sent_at: row.try_get("sent_at").map_err(|e| col_err("sent_at", e))?,
    })
}

fn parse_channel_row(row: &PgRow) -> Result<NotificationChannelRecord, CoreError> {
    let channel_type_raw: String = row
        .try_get("channel_type")
        .map_err(|e| col_err("channel_type", e))?;
    Ok(NotificationChannelRecord {
        id: row.try_get("id").map_err(|e| col_err("id", e))?,
        user_id: row.try_get("user_id").map_err(|e| col_err("user_id", e))?,
        channel_type: channel_type_raw.parse::<NotificationChannelType>()?,
        name: row.try_get("name").map_err(|e| col_err("name", e))?,
        enabled: row.try_get("enabled").map_err(|e| col_err("enabled", e))?,
        config_enc: row
            .try_get("config_enc")
            .map_err(|e| col_err("config_enc", e))?,
        config_hint: row
            .try_get("config_hint")
            .map_err(|e| col_err("config_hint", e))?,
        trigger_on_daily: row
            .try_get("trigger_on_daily")
            .map_err(|e| col_err("trigger_on_daily", e))?,
        trigger_on_alert: row
            .try_get("trigger_on_alert")
            .map_err(|e| col_err("trigger_on_alert", e))?,
        created_at: row
            .try_get("created_at")
            .map_err(|e| col_err("created_at", e))?,
    })
}

fn parse_schedule_row(row: &PgRow) -> Result<UserScheduleRecord, CoreError> {
    Ok(UserScheduleRecord {
        user_id: row.try_get("user_id").map_err(|e| col_err("user_id", e))?,
        timezone: row
            .try_get("timezone")
            .map_err(|e| col_err("timezone", e))?,
        daily_time: row
            .try_get("daily_time")
            .map_err(|e| col_err("daily_time", e))?,
        email_enabled: row
            .try_get("email_enabled")
            .map_err(|e| col_err("email_enabled", e))?,
        send_when_empty: row
            .try_get("send_when_empty")
            .map_err(|e| col_err("send_when_empty", e))?,
        alert_scan_enabled: row
            .try_get("alert_scan_enabled")
            .map_err(|e| col_err("alert_scan_enabled", e))?,
        alert_interval_minutes: row
            .try_get("alert_interval_minutes")
            .map_err(|e| col_err("alert_interval_minutes", e))?,
        last_daily_run_date: row
            .try_get("last_daily_run_date")
            .map_err(|e| col_err("last_daily_run_date", e))?,
        last_alert_scan_at: row
            .try_get("last_alert_scan_at")
            .map_err(|e| col_err("last_alert_scan_at", e))?,
    })
}

fn parse_watchlist_rule_row(row: &PgRow) -> Result<WatchlistRuleRecord, CoreError> {
    let priority_raw: String = row
        .try_get("priority")
        .map_err(|e| col_err("priority", e))?;
    Ok(WatchlistRuleRecord {
        id: row.try_get("id").map_err(|e| col_err("id", e))?,
        user_id: row.try_get("user_id").map_err(|e| col_err("user_id", e))?,
        name: row.try_get("name").map_err(|e| col_err("name", e))?,
        keywords: row
            .try_get("keywords")
            .map_err(|e| col_err("keywords", e))?,
        exclude_keywords: row
            .try_get("exclude_keywords")
            .map_err(|e| col_err("exclude_keywords", e))?,
        priority: priority_raw.parse::<AlertPriority>()?,
        notify_immediately: row
            .try_get("notify_immediately")
            .map_err(|e| col_err("notify_immediately", e))?,
        enabled: row.try_get("enabled").map_err(|e| col_err("enabled", e))?,
        created_at: row
            .try_get("created_at")
            .map_err(|e| col_err("created_at", e))?,
    })
}

#[async_trait]
impl UserRepo for PostgresRepoStore {
    async fn upsert_oidc_user(&self, cmd: UpsertUserCmd) -> Result<UserRecord, CoreError> {
        let oidc_sub = cmd.oidc_sub.trim();
        if oidc_sub.is_empty() {
            return Err(CoreError::InvalidInput(
                "oidc_sub must not be empty".to_owned(),
            ));
        }
        let role = if cmd.role.trim().is_empty() {
            crate::constants::DEFAULT_USER_ROLE
        } else {
            cmd.role.trim()
        };

        let mut tx = self.pool.begin().await.map_err(map_pg_err)?;

        let row = sqlx::query(
            r#"
            INSERT INTO users (id, oidc_sub, email, display_name, role, created_at, last_login_at)
            VALUES ($1, $2, $3, $4, $5, NOW(), NOW())
            ON CONFLICT (oidc_sub) DO UPDATE SET
                email = COALESCE(EXCLUDED.email, users.email),
                display_name = COALESCE(EXCLUDED.display_name, users.display_name),
                last_login_at = NOW()
            RETURNING id, oidc_sub, email, display_name, role, created_at, last_login_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(oidc_sub)
        .bind(cmd.email.as_deref())
        .bind(cmd.display_name.as_deref())
        .bind(role)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        let user = parse_user_row(&row)?;
        let default_interval = i32::try_from(DEFAULT_ALERT_INTERVAL_MINUTES).unwrap_or(60);

        sqlx::query(
            r#"
            INSERT INTO user_schedules (
                user_id, timezone, daily_time, email_enabled,
                send_when_empty, alert_scan_enabled, alert_interval_minutes
            )
            VALUES ($1, $2, $3, TRUE, FALSE, TRUE, $4)
            ON CONFLICT (user_id) DO NOTHING
            "#,
        )
        .bind(user.id)
        .bind(DEFAULT_TIMEZONE)
        .bind(DEFAULT_DAILY_TIME)
        .bind(default_interval)
        .execute(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        tx.commit().await.map_err(map_pg_err)?;
        Ok(user)
    }

    async fn get_user_by_id(&self, user_id: Uuid) -> Result<Option<UserRecord>, CoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, oidc_sub, email, display_name, role, created_at, last_login_at
            FROM users
            WHERE id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_pg_err)?;

        row.as_ref().map(parse_user_row).transpose()
    }

    async fn get_onboarding_flags(&self, user_id: Uuid) -> Result<OnboardingFlags, CoreError> {
        let row = sqlx::query(
            r#"
            SELECT
                EXISTS(SELECT 1 FROM user_ai_configs WHERE user_id = $1) AS has_ai_config,
                EXISTS(SELECT 1 FROM user_subscriptions WHERE user_id = $1) AS has_sources,
                EXISTS(SELECT 1 FROM user_notification_channels WHERE user_id = $1) AS has_channels,
                EXISTS(SELECT 1 FROM user_schedules WHERE user_id = $1) AS has_schedule
            "#,
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        Ok(OnboardingFlags {
            has_ai_config: row
                .try_get("has_ai_config")
                .map_err(|e| col_err("has_ai_config", e))?,
            has_sources: row
                .try_get("has_sources")
                .map_err(|e| col_err("has_sources", e))?,
            has_channels: row
                .try_get("has_channels")
                .map_err(|e| col_err("has_channels", e))?,
            has_schedule: row
                .try_get("has_schedule")
                .map_err(|e| col_err("has_schedule", e))?,
        })
    }
}

const USER_SOURCE_SELECT_SQL: &str = r#"
SELECT
    s.id AS subscription_id,
    c.id AS canonical_source_id,
    s.user_id,
    c.source_type,
    c.scope,
    c.scope_owner_id,
    c.canonical_key,
    c.default_name,
    s.custom_name,
    s.category,
    s.enabled,
    c.config_json_enc,
    c.etag,
    c.last_modified,
    c.last_fetched_at,
    c.next_poll_at,
    c.last_status,
    c.last_error,
    c.consecutive_failures,
    s.created_at
FROM user_subscriptions s
INNER JOIN canonical_sources c ON c.id = s.canonical_source_id
"#;

#[async_trait]
impl SourceRepo for PostgresRepoStore {
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
        let mut tx = self.pool.begin().await.map_err(map_pg_err)?;

        let canonical_source_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO canonical_sources (
                id, source_type, canonical_key, scope, scope_owner_id,
                default_name, config_json_enc, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())
            ON CONFLICT (source_type, canonical_key, scope_owner_id) DO UPDATE SET
                config_json_enc = CASE
                    WHEN canonical_sources.scope = 'user_private'
                        THEN COALESCE(EXCLUDED.config_json_enc, canonical_sources.config_json_enc)
                    ELSE COALESCE(canonical_sources.config_json_enc, EXCLUDED.config_json_enc)
                END
            RETURNING id
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(cmd.source_type.as_str())
        .bind(canonical_key)
        .bind(cmd.scope.as_str())
        .bind(scope_owner_id)
        .bind(default_name)
        .bind(cmd.config_json_enc.as_deref())
        .fetch_one(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        let subscription_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO user_subscriptions (
                id, user_id, canonical_source_id, custom_name, category, enabled, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, NOW())
            ON CONFLICT (user_id, canonical_source_id) DO UPDATE SET
                custom_name = EXCLUDED.custom_name,
                category = EXCLUDED.category,
                enabled = EXCLUDED.enabled
            RETURNING id
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(canonical_source_id)
        .bind(custom_name)
        .bind(category)
        .bind(cmd.enabled)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        let joined_sql = format!(
            "{USER_SOURCE_SELECT_SQL} WHERE s.id = $1 AND s.user_id = $2 AND (c.scope = 'shared_public' OR c.scope_owner_id = $2)"
        );
        let row = sqlx::query(&joined_sql)
            .bind(subscription_id)
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_pg_err)?;

        tx.commit().await.map_err(map_pg_err)?;

        let rec = parse_user_source_row(&row)?;
        self.record_to_source_view(&rec)
    }

    async fn list_user_sources(&self, user_id: Uuid) -> Result<Vec<UserSourceView>, CoreError> {
        let sql = format!(
            "{USER_SOURCE_SELECT_SQL} WHERE s.user_id = $1 AND (c.scope = 'shared_public' OR c.scope_owner_id = $1) ORDER BY s.created_at ASC, s.id ASC"
        );
        let rows = sqlx::query(&sql)
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(map_pg_err)?;

        rows.iter()
            .map(|row| {
                let rec = parse_user_source_row(row)?;
                self.record_to_source_view(&rec)
            })
            .collect()
    }

    async fn get_user_source(
        &self,
        user_id: Uuid,
        id_or_sub: Uuid,
    ) -> Result<Option<UserSourceRecord>, CoreError> {
        let sql = format!(
            "{USER_SOURCE_SELECT_SQL} WHERE s.user_id = $1 AND (s.canonical_source_id = $2 OR s.id = $2) AND (c.scope = 'shared_public' OR c.scope_owner_id = $1)"
        );
        let row = sqlx::query(&sql)
            .bind(user_id)
            .bind(id_or_sub)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_pg_err)?;

        row.as_ref().map(parse_user_source_row).transpose()
    }

    async fn update_user_subscription(
        &self,
        user_id: Uuid,
        id_or_sub: Uuid,
        cmd: UpdateSourceCmd,
    ) -> Result<UserSourceView, CoreError> {
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

        let mut tx = self.pool.begin().await.map_err(map_pg_err)?;

        let existing = sqlx::query(
            r#"
            SELECT s.id AS subscription_id, c.id AS canonical_source_id, c.scope, c.scope_owner_id
            FROM user_subscriptions s
            INNER JOIN canonical_sources c ON c.id = s.canonical_source_id
            WHERE s.user_id = $1
              AND (s.canonical_source_id = $2 OR s.id = $2)
              AND (c.scope = 'shared_public' OR c.scope_owner_id = $1)
            "#,
        )
        .bind(user_id)
        .bind(id_or_sub)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_pg_err)?
        .ok_or_else(|| CoreError::NotFound("source subscription not found".to_owned()))?;

        let sub_id: Uuid = existing
            .try_get("subscription_id")
            .map_err(|e| col_err("subscription_id", e))?;
        let src_id: Uuid = existing
            .try_get("canonical_source_id")
            .map_err(|e| col_err("canonical_source_id", e))?;
        let scope_raw: String = existing.try_get("scope").map_err(|e| col_err("scope", e))?;
        let scope = scope_raw.parse::<SourceScope>()?;
        let scope_owner_id: Option<Uuid> = existing
            .try_get("scope_owner_id")
            .map_err(|e| col_err("scope_owner_id", e))?;

        if scope == SourceScope::SharedPublic
            && (cmd.config_json_enc.is_some() || cmd.handle_or_url.is_some())
        {
            return Err(CoreError::Forbidden(
                "shared_public canonical source config cannot be modified per-user".to_owned(),
            ));
        }

        sqlx::query(
            r#"
            UPDATE user_subscriptions
            SET custom_name = COALESCE($2, custom_name),
                category = COALESCE($3, category),
                enabled = COALESCE($4, enabled)
            WHERE id = $1 AND user_id = $5
            "#,
        )
        .bind(sub_id)
        .bind(cmd.custom_name.as_deref().map(str::trim))
        .bind(cmd.category.as_deref().map(str::trim))
        .bind(cmd.enabled)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        if scope == SourceScope::UserPrivate && scope_owner_id == Some(user_id) {
            if let Some(ref enc) = cmd.config_json_enc {
                sqlx::query(
                    r#"
                    UPDATE canonical_sources
                    SET config_json_enc = $2
                    WHERE id = $1 AND scope = 'user_private' AND scope_owner_id = $3
                    "#,
                )
                .bind(src_id)
                .bind(enc)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_err(map_pg_err)?;
            }
        }

        let joined_sql = format!("{USER_SOURCE_SELECT_SQL} WHERE s.id = $1 AND s.user_id = $2");
        let row = sqlx::query(&joined_sql)
            .bind(sub_id)
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_pg_err)?;

        tx.commit().await.map_err(map_pg_err)?;
        let rec = parse_user_source_row(&row)?;
        self.record_to_source_view(&rec)
    }

    async fn delete_user_subscription(
        &self,
        user_id: Uuid,
        id_or_sub: Uuid,
    ) -> Result<(), CoreError> {
        let mut tx = self.pool.begin().await.map_err(map_pg_err)?;

        let existing = sqlx::query(
            r#"
            SELECT s.id AS subscription_id, c.id AS canonical_source_id, c.scope, c.scope_owner_id
            FROM user_subscriptions s
            INNER JOIN canonical_sources c ON c.id = s.canonical_source_id
            WHERE s.user_id = $1
              AND (s.canonical_source_id = $2 OR s.id = $2)
              AND (c.scope = 'shared_public' OR c.scope_owner_id = $1)
            "#,
        )
        .bind(user_id)
        .bind(id_or_sub)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_pg_err)?
        .ok_or_else(|| CoreError::NotFound("source subscription not found".to_owned()))?;

        let sub_id: Uuid = existing
            .try_get("subscription_id")
            .map_err(|e| col_err("subscription_id", e))?;
        let src_id: Uuid = existing
            .try_get("canonical_source_id")
            .map_err(|e| col_err("canonical_source_id", e))?;
        let scope_raw: String = existing.try_get("scope").map_err(|e| col_err("scope", e))?;
        let scope = scope_raw.parse::<SourceScope>()?;
        let scope_owner_id: Option<Uuid> = existing
            .try_get("scope_owner_id")
            .map_err(|e| col_err("scope_owner_id", e))?;

        sqlx::query("DELETE FROM user_subscriptions WHERE id = $1 AND user_id = $2")
            .bind(sub_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(map_pg_err)?;

        if scope == SourceScope::UserPrivate && scope_owner_id == Some(user_id) {
            sqlx::query(
                "DELETE FROM canonical_sources WHERE id = $1 AND scope = 'user_private' AND scope_owner_id = $2",
            )
            .bind(src_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(map_pg_err)?;
        }

        tx.commit().await.map_err(map_pg_err)?;
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
        let rows = sqlx::query(
            r#"
            SELECT
                c.id, c.source_type, c.canonical_key, c.scope, c.scope_owner_id,
                c.default_name, c.config_json_enc, c.etag, c.last_modified,
                c.last_fetched_at, c.next_poll_at, c.last_status, c.last_error,
                c.consecutive_failures, c.created_at
            FROM canonical_sources c
            WHERE EXISTS (
                SELECT 1 FROM user_subscriptions s
                WHERE s.canonical_source_id = c.id
                  AND s.enabled = TRUE
            )
              AND ($1::TEXT IS NULL OR c.scope = $1)
              AND (c.next_poll_at IS NULL OR c.next_poll_at <= $2)
            ORDER BY c.next_poll_at ASC NULLS FIRST, c.last_fetched_at ASC NULLS FIRST, c.created_at ASC
            LIMIT $3
            "#,
        )
        .bind(scope.map(SourceScope::as_str))
        .bind(due_before)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        rows.iter().map(parse_canonical_source_row).collect()
    }

    async fn record_fetch_outcome(
        &self,
        source_id: Uuid,
        update: SourceFetchStateUpdate,
    ) -> Result<(), CoreError> {
        let res = sqlx::query(
            r#"
            UPDATE canonical_sources
            SET last_status = $2,
                etag = COALESCE($3, etag),
                last_modified = COALESCE($4, last_modified),
                last_fetched_at = $5,
                next_poll_at = $6,
                last_error = $7,
                consecutive_failures = $8
            WHERE id = $1
            "#,
        )
        .bind(source_id)
        .bind(update.outcome.as_str())
        .bind(update.etag.as_deref())
        .bind(update.last_modified.as_deref())
        .bind(update.fetched_at)
        .bind(update.next_poll_at)
        .bind(update.last_error.as_deref())
        .bind(update.consecutive_failures)
        .execute(&self.pool)
        .await
        .map_err(map_pg_err)?;

        if res.rows_affected() == 0 {
            return Err(CoreError::NotFound("canonical source not found".to_owned()));
        }
        Ok(())
    }

    async fn upsert_canonical_items(
        &self,
        source_id: Uuid,
        items: &[NewCanonicalItem],
    ) -> Result<Vec<CanonicalItemRecord>, CoreError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = self.pool.begin().await.map_err(map_pg_err)?;
        let source_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM canonical_sources WHERE id = $1)")
                .bind(source_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_pg_err)?;
        if !source_exists {
            return Err(CoreError::NotFound("canonical source not found".to_owned()));
        }

        let mut inserted = Vec::new();
        for item in items {
            let row = sqlx::query(
                r#"
                INSERT INTO canonical_items (
                    id, canonical_source_id, external_id, canonical_url, content_hash,
                    title, author, summary_raw, body_pruned, published_at, fetched_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, FALSE, $9, NOW())
                ON CONFLICT DO NOTHING
                RETURNING id, canonical_source_id, external_id, canonical_url, content_hash,
                          title, author, summary_raw, body_pruned, published_at, fetched_at
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(source_id)
            .bind(item.external_id.as_deref())
            .bind(&item.canonical_url)
            .bind(&item.content_hash)
            .bind(&item.title)
            .bind(item.author.as_deref())
            .bind(item.summary_raw.as_deref())
            .bind(item.published_at)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_pg_err)?;

            if let Some(ref r) = row {
                inserted.push(parse_canonical_item_row(r)?);
            }
        }

        tx.commit().await.map_err(map_pg_err)?;
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
        let rows = sqlx::query(
            r#"
            SELECT id, canonical_source_id, external_id, canonical_url, content_hash,
                   title, author, summary_raw, body_pruned, published_at, fetched_at
            FROM canonical_items
            WHERE canonical_source_id = $1
              AND COALESCE(published_at, fetched_at) >= $2
            ORDER BY COALESCE(published_at, fetched_at) DESC, fetched_at DESC
            LIMIT $3
            "#,
        )
        .bind(source_id)
        .bind(since)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        rows.iter().map(parse_canonical_item_row).collect()
    }

    async fn list_source_items(
        &self,
        user_id: Uuid,
        source_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<ItemView>, CoreError> {
        let sub_row = sqlx::query(
            r#"
            SELECT s.canonical_source_id
            FROM user_subscriptions s
            INNER JOIN canonical_sources c ON c.id = s.canonical_source_id
            WHERE s.user_id = $1
              AND (s.canonical_source_id = $2 OR s.id = $2)
              AND (c.scope = 'shared_public' OR c.scope_owner_id = $1)
            "#,
        )
        .bind(user_id)
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_pg_err)?
        .ok_or_else(|| CoreError::NotFound("source subscription not found".to_owned()))?;

        let canonical_source_id: Uuid = sub_row
            .try_get("canonical_source_id")
            .map_err(|e| col_err("canonical_source_id", e))?;

        let total_i64: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM canonical_items WHERE canonical_source_id = $1",
        )
        .bind(canonical_source_id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;
        let total = u64::try_from(total_i64.max(0)).unwrap_or(0);

        let rows = sqlx::query(
            r#"
            SELECT
                i.id,
                s.canonical_source_id AS source_id,
                s.custom_name AS source_name,
                s.category,
                i.external_id,
                i.canonical_url,
                i.title,
                i.author,
                i.summary_raw AS summary,
                i.body_pruned,
                i.published_at,
                i.fetched_at,
                COALESCE(m.matched_keywords, '{}') AS matched_keywords,
                COALESCE(m.priority, 'normal') AS priority,
                COALESCE(m.importance_score, 0) AS importance_score,
                (b.id IS NOT NULL) AS bookmarked,
                b.note AS bookmark_note
            FROM canonical_items i
            INNER JOIN user_subscriptions s
                ON s.canonical_source_id = i.canonical_source_id
               AND s.user_id = $1
            LEFT JOIN user_item_matches m
                ON m.item_id = i.id
               AND m.user_id = $1
            LEFT JOIN user_bookmarks b
                ON b.item_id = i.id
               AND b.user_id = $1
            WHERE i.canonical_source_id = $2
            ORDER BY COALESCE(i.published_at, i.fetched_at) DESC, i.fetched_at DESC, i.id DESC
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind(user_id)
        .bind(canonical_source_id)
        .bind(i64::try_from(page.limit()).unwrap_or(i64::MAX))
        .bind(i64::try_from(page.offset()).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        let items = rows
            .iter()
            .map(parse_item_view_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PagedResult::new(items, page, total))
    }

    async fn search_user_items(
        &self,
        user_id: Uuid,
        query: ItemSearchQuery,
    ) -> Result<PagedResult<ItemView>, CoreError> {
        let q_filter = query.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let kw_filter = query
            .keyword
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let date_from_dt = query
            .date_from
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| dt.and_utc());
        let date_to_excl_dt = query
            .date_to
            .and_then(|d| d.succ_opt())
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| dt.and_utc());

        let total_i64: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM canonical_items i
            INNER JOIN user_subscriptions s
                ON s.canonical_source_id = i.canonical_source_id
               AND s.user_id = $1
            INNER JOIN canonical_sources c
                ON c.id = i.canonical_source_id
               AND (c.scope = 'shared_public' OR c.scope_owner_id = $1)
            LEFT JOIN user_item_matches m
                ON m.item_id = i.id
               AND m.user_id = $1
            LEFT JOIN user_bookmarks b
                ON b.item_id = i.id
               AND b.user_id = $1
            WHERE ($2::TEXT IS NULL OR i.search_tsv @@ (
                CASE
                    WHEN numnode(websearch_to_tsquery('simple', COALESCE($2, ''))) > 0
                        THEN websearch_to_tsquery('simple', COALESCE($2, ''))
                    ELSE plainto_tsquery('simple', COALESCE($2, ''))
                END
            ))
              AND ($3::UUID IS NULL OR i.canonical_source_id = $3 OR s.id = $3)
              AND ($4::TIMESTAMPTZ IS NULL OR COALESCE(i.published_at, i.fetched_at) >= $4)
              AND ($5::TIMESTAMPTZ IS NULL OR COALESCE(i.published_at, i.fetched_at) < $5)
              AND ($6::TEXT IS NULL OR EXISTS (
                  SELECT 1
                  FROM unnest(COALESCE(m.matched_keywords, '{}')) AS kw
                  WHERE LOWER(kw) = LOWER($6)
              ))
              AND ($7::BOOL IS NULL OR (b.id IS NOT NULL) = $7)
            "#,
        )
        .bind(user_id)
        .bind(q_filter)
        .bind(query.source_id)
        .bind(date_from_dt)
        .bind(date_to_excl_dt)
        .bind(kw_filter)
        .bind(query.bookmarked)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;
        let total = u64::try_from(total_i64.max(0)).unwrap_or(0);

        let rows = sqlx::query(
            r#"
            SELECT
                i.id,
                s.canonical_source_id AS source_id,
                s.custom_name AS source_name,
                s.category,
                i.external_id,
                i.canonical_url,
                i.title,
                i.author,
                i.summary_raw AS summary,
                i.body_pruned,
                i.published_at,
                i.fetched_at,
                COALESCE(m.matched_keywords, '{}') AS matched_keywords,
                COALESCE(m.priority, 'normal') AS priority,
                COALESCE(m.importance_score, 0) AS importance_score,
                (b.id IS NOT NULL) AS bookmarked,
                b.note AS bookmark_note
            FROM canonical_items i
            INNER JOIN user_subscriptions s
                ON s.canonical_source_id = i.canonical_source_id
               AND s.user_id = $1
            INNER JOIN canonical_sources c
                ON c.id = i.canonical_source_id
               AND (c.scope = 'shared_public' OR c.scope_owner_id = $1)
            LEFT JOIN user_item_matches m
                ON m.item_id = i.id
               AND m.user_id = $1
            LEFT JOIN user_bookmarks b
                ON b.item_id = i.id
               AND b.user_id = $1
            WHERE ($2::TEXT IS NULL OR i.search_tsv @@ (
                CASE
                    WHEN numnode(websearch_to_tsquery('simple', COALESCE($2, ''))) > 0
                        THEN websearch_to_tsquery('simple', COALESCE($2, ''))
                    ELSE plainto_tsquery('simple', COALESCE($2, ''))
                END
            ))
              AND ($3::UUID IS NULL OR i.canonical_source_id = $3 OR s.id = $3)
              AND ($4::TIMESTAMPTZ IS NULL OR COALESCE(i.published_at, i.fetched_at) >= $4)
              AND ($5::TIMESTAMPTZ IS NULL OR COALESCE(i.published_at, i.fetched_at) < $5)
              AND ($6::TEXT IS NULL OR EXISTS (
                  SELECT 1
                  FROM unnest(COALESCE(m.matched_keywords, '{}')) AS kw
                  WHERE LOWER(kw) = LOWER($6)
              ))
              AND ($7::BOOL IS NULL OR (b.id IS NOT NULL) = $7)
            ORDER BY COALESCE(i.published_at, i.fetched_at) DESC, i.fetched_at DESC, i.id DESC
            LIMIT $8 OFFSET $9
            "#,
        )
        .bind(user_id)
        .bind(q_filter)
        .bind(query.source_id)
        .bind(date_from_dt)
        .bind(date_to_excl_dt)
        .bind(kw_filter)
        .bind(query.bookmarked)
        .bind(i64::try_from(query.page.limit()).unwrap_or(i64::MAX))
        .bind(i64::try_from(query.page.offset()).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        let items = rows
            .iter()
            .map(parse_item_view_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PagedResult::new(items, query.page, total))
    }

    async fn upsert_user_item_matches(
        &self,
        user_id: Uuid,
        matches: &[NewUserItemMatch],
    ) -> Result<(), CoreError> {
        if matches.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await.map_err(map_pg_err)?;
        for m in matches {
            sqlx::query(
                r#"
                INSERT INTO user_item_matches (
                    id, user_id, item_id, matched_keywords, priority, importance_score
                )
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (user_id, item_id) DO UPDATE SET
                    matched_keywords = EXCLUDED.matched_keywords,
                    priority = EXCLUDED.priority,
                    importance_score = EXCLUDED.importance_score
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(user_id)
            .bind(m.item_id)
            .bind(&m.matched_keywords)
            .bind(m.priority.as_str())
            .bind(m.importance_score)
            .execute(&mut *tx)
            .await
            .map_err(map_pg_err)?;
        }
        tx.commit().await.map_err(map_pg_err)?;
        Ok(())
    }

    async fn list_pending_urgent_alerts(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<PendingAlertItem>, CoreError> {
        let rows = sqlx::query(
            r#"
            SELECT
                i.id AS item_id,
                s.canonical_source_id AS source_id,
                s.custom_name AS source_name,
                s.category,
                i.canonical_url,
                i.title,
                i.author,
                i.summary_raw AS summary,
                i.published_at,
                m.matched_keywords,
                m.priority,
                m.importance_score
            FROM user_item_matches m
            INNER JOIN canonical_items i ON i.id = m.item_id
            INNER JOIN user_subscriptions s
                ON s.canonical_source_id = i.canonical_source_id
               AND s.user_id = $1
               AND s.enabled = TRUE
            INNER JOIN canonical_sources c
                ON c.id = i.canonical_source_id
               AND (c.scope = 'shared_public' OR c.scope_owner_id = $1)
            WHERE m.user_id = $1
              AND m.priority = $2
              AND m.alerted_at IS NULL
            ORDER BY m.importance_score DESC, COALESCE(i.published_at, i.fetched_at) DESC
            "#,
        )
        .bind(user_id)
        .bind(AlertPriority::Urgent.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        rows.iter()
            .map(|row| {
                let priority_raw: String = row
                    .try_get("priority")
                    .map_err(|e| col_err("priority", e))?;
                Ok(PendingAlertItem {
                    item_id: row.try_get("item_id").map_err(|e| col_err("item_id", e))?,
                    source_id: row
                        .try_get("source_id")
                        .map_err(|e| col_err("source_id", e))?,
                    source_name: row
                        .try_get("source_name")
                        .map_err(|e| col_err("source_name", e))?,
                    category: row
                        .try_get("category")
                        .map_err(|e| col_err("category", e))?,
                    canonical_url: row
                        .try_get("canonical_url")
                        .map_err(|e| col_err("canonical_url", e))?,
                    title: row.try_get("title").map_err(|e| col_err("title", e))?,
                    author: row.try_get("author").map_err(|e| col_err("author", e))?,
                    summary: row.try_get("summary").map_err(|e| col_err("summary", e))?,
                    published_at: row
                        .try_get("published_at")
                        .map_err(|e| col_err("published_at", e))?,
                    matched_keywords: row
                        .try_get("matched_keywords")
                        .map_err(|e| col_err("matched_keywords", e))?,
                    priority: priority_raw.parse::<AlertPriority>()?,
                    importance_score: row
                        .try_get("importance_score")
                        .map_err(|e| col_err("importance_score", e))?,
                })
            })
            .collect()
    }

    async fn mark_items_alerted(
        &self,
        user_id: Uuid,
        item_ids: &[Uuid],
        alerted_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        if item_ids.is_empty() {
            return Ok(());
        }
        sqlx::query(
            r#"
            UPDATE user_item_matches
            SET alerted_at = $1
            WHERE user_id = $2
              AND item_id = ANY($3)
              AND alerted_at IS NULL
            "#,
        )
        .bind(alerted_at)
        .bind(user_id)
        .bind(item_ids)
        .execute(&self.pool)
        .await
        .map_err(map_pg_err)?;

        Ok(())
    }

    async fn prune_unbookmarked_bodies(&self, cutoff: DateTime<Utc>) -> Result<u64, CoreError> {
        let res = sqlx::query(
            r#"
            UPDATE canonical_items
            SET summary_raw = NULL,
                body_pruned = TRUE
            WHERE body_pruned = FALSE
              AND fetched_at < $1
              AND NOT EXISTS (
                  SELECT 1 FROM user_bookmarks b
                  WHERE b.item_id = canonical_items.id
              )
            "#,
        )
        .bind(cutoff)
        .execute(&self.pool)
        .await
        .map_err(map_pg_err)?;

        Ok(res.rows_affected())
    }

    async fn upsert_bookmark(
        &self,
        user_id: Uuid,
        item_id: Uuid,
        note: Option<String>,
    ) -> Result<BookmarkView, CoreError> {
        let accessible: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM canonical_items i
                INNER JOIN canonical_sources c ON c.id = i.canonical_source_id
                WHERE i.id = $2
                  AND (
                      c.scope = 'shared_public'
                      OR (c.scope = 'user_private' AND c.scope_owner_id = $1)
                  )
            )
            "#,
        )
        .bind(user_id)
        .bind(item_id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        if !accessible {
            return Err(CoreError::NotFound("item not found".to_owned()));
        }

        let row = sqlx::query(
            r#"
            INSERT INTO user_bookmarks (id, user_id, item_id, note, created_at)
            VALUES ($1, $2, $3, $4, NOW())
            ON CONFLICT (user_id, item_id) DO UPDATE SET
                note = EXCLUDED.note
            RETURNING id, item_id, note, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(item_id)
        .bind(note.as_deref())
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        Ok(BookmarkView {
            id: row.try_get("id").map_err(|e| col_err("id", e))?,
            item_id: row.try_get("item_id").map_err(|e| col_err("item_id", e))?,
            note: row.try_get("note").map_err(|e| col_err("note", e))?,
            created_at: row
                .try_get("created_at")
                .map_err(|e| col_err("created_at", e))?,
        })
    }

    async fn delete_bookmark(&self, user_id: Uuid, item_id: Uuid) -> Result<(), CoreError> {
        let res = sqlx::query("DELETE FROM user_bookmarks WHERE user_id = $1 AND item_id = $2")
            .bind(user_id)
            .bind(item_id)
            .execute(&self.pool)
            .await
            .map_err(map_pg_err)?;

        if res.rows_affected() == 0 {
            return Err(CoreError::NotFound("bookmark not found".to_owned()));
        }
        Ok(())
    }
}

#[async_trait]
impl DigestRepo for PostgresRepoStore {
    async fn list_daily_digests(
        &self,
        user_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<DailyDigestSummary>, CoreError> {
        let total_i64: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM daily_digests WHERE user_id = $1")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await
                .map_err(map_pg_err)?;
        let total = u64::try_from(total_i64.max(0)).unwrap_or(0);

        let rows = sqlx::query(
            r#"
            SELECT id, digest_date, title, item_count, highlighted_count,
                   prompt_tokens, completion_tokens, model_used, degraded_fallback, created_at
            FROM daily_digests
            WHERE user_id = $1
            ORDER BY digest_date DESC, created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(user_id)
        .bind(i64::try_from(page.limit()).unwrap_or(i64::MAX))
        .bind(i64::try_from(page.offset()).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        let items = rows
            .iter()
            .map(|row| {
                Ok(DailyDigestSummary {
                    id: row.try_get("id").map_err(|e| col_err("id", e))?,
                    digest_date: row
                        .try_get("digest_date")
                        .map_err(|e| col_err("digest_date", e))?,
                    title: row.try_get("title").map_err(|e| col_err("title", e))?,
                    item_count: row
                        .try_get("item_count")
                        .map_err(|e| col_err("item_count", e))?,
                    highlighted_count: row
                        .try_get("highlighted_count")
                        .map_err(|e| col_err("highlighted_count", e))?,
                    prompt_tokens: row
                        .try_get("prompt_tokens")
                        .map_err(|e| col_err("prompt_tokens", e))?,
                    completion_tokens: row
                        .try_get("completion_tokens")
                        .map_err(|e| col_err("completion_tokens", e))?,
                    model_used: row
                        .try_get("model_used")
                        .map_err(|e| col_err("model_used", e))?,
                    degraded_fallback: row
                        .try_get("degraded_fallback")
                        .map_err(|e| col_err("degraded_fallback", e))?,
                    created_at: row
                        .try_get("created_at")
                        .map_err(|e| col_err("created_at", e))?,
                })
            })
            .collect::<Result<Vec<_>, CoreError>>()?;

        Ok(PagedResult::new(items, page, total))
    }

    async fn get_daily_digest_by_date(
        &self,
        user_id: Uuid,
        date: NaiveDate,
    ) -> Result<Option<DailyDigestDetail>, CoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, digest_date, input_fingerprint, title, overview_markdown, overview_html,
                   sections_json, item_count, highlighted_count, prompt_tokens,
                   completion_tokens, model_used, degraded_fallback, created_at
            FROM daily_digests
            WHERE user_id = $1 AND digest_date = $2
            "#,
        )
        .bind(user_id)
        .bind(date)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_pg_err)?;

        row.as_ref().map(parse_daily_digest_detail_row).transpose()
    }

    async fn find_daily_digest_by_fingerprint(
        &self,
        user_id: Uuid,
        date: NaiveDate,
        fingerprint: &str,
    ) -> Result<Option<DailyDigestDetail>, CoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, digest_date, input_fingerprint, title, overview_markdown, overview_html,
                   sections_json, item_count, highlighted_count, prompt_tokens,
                   completion_tokens, model_used, degraded_fallback, created_at
            FROM daily_digests
            WHERE user_id = $1 AND digest_date = $2 AND input_fingerprint = $3
            "#,
        )
        .bind(user_id)
        .bind(date)
        .bind(fingerprint)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_pg_err)?;

        row.as_ref().map(parse_daily_digest_detail_row).transpose()
    }

    async fn save_daily_and_source_digests(
        &self,
        cmd: SaveDigestCmd,
    ) -> Result<DailyDigestDetail, CoreError> {
        let mut tx = self.pool.begin().await.map_err(map_pg_err)?;

        let row = sqlx::query(
            r#"
            INSERT INTO daily_digests (
                id, user_id, digest_date, input_fingerprint, title, overview_markdown,
                overview_html, sections_json, item_count, highlighted_count,
                prompt_tokens, completion_tokens, model_used, degraded_fallback, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, NOW())
            ON CONFLICT (user_id, digest_date) DO UPDATE SET
                input_fingerprint = EXCLUDED.input_fingerprint,
                title = EXCLUDED.title,
                overview_markdown = EXCLUDED.overview_markdown,
                overview_html = EXCLUDED.overview_html,
                sections_json = EXCLUDED.sections_json,
                item_count = EXCLUDED.item_count,
                highlighted_count = EXCLUDED.highlighted_count,
                prompt_tokens = EXCLUDED.prompt_tokens,
                completion_tokens = EXCLUDED.completion_tokens,
                model_used = EXCLUDED.model_used,
                degraded_fallback = EXCLUDED.degraded_fallback,
                created_at = NOW()
            RETURNING id, digest_date, input_fingerprint, title, overview_markdown, overview_html,
                      sections_json, item_count, highlighted_count, prompt_tokens,
                      completion_tokens, model_used, degraded_fallback, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(cmd.user_id)
        .bind(cmd.digest_date)
        .bind(&cmd.input_fingerprint)
        .bind(&cmd.title)
        .bind(&cmd.overview_markdown)
        .bind(&cmd.overview_html)
        .bind(Json(cmd.sections))
        .bind(cmd.item_count)
        .bind(cmd.highlighted_count)
        .bind(cmd.prompt_tokens)
        .bind(cmd.completion_tokens)
        .bind(&cmd.model_used)
        .bind(cmd.degraded_fallback)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        let detail = parse_daily_digest_detail_row(&row)?;

        for sd in cmd.source_digests {
            sqlx::query(
                r#"
                INSERT INTO source_daily_digests (
                    id, user_id, digest_id, canonical_source_id, digest_date,
                    input_fingerprint, summary_markdown, key_points_json, item_count, created_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
                ON CONFLICT (user_id, canonical_source_id, digest_date) DO UPDATE SET
                    digest_id = EXCLUDED.digest_id,
                    input_fingerprint = EXCLUDED.input_fingerprint,
                    summary_markdown = EXCLUDED.summary_markdown,
                    key_points_json = EXCLUDED.key_points_json,
                    item_count = EXCLUDED.item_count,
                    created_at = NOW()
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(cmd.user_id)
            .bind(detail.id)
            .bind(sd.canonical_source_id)
            .bind(sd.digest_date)
            .bind(&sd.input_fingerprint)
            .bind(&sd.summary_markdown)
            .bind(Json(sd.key_points))
            .bind(sd.item_count)
            .execute(&mut *tx)
            .await
            .map_err(map_pg_err)?;
        }

        tx.commit().await.map_err(map_pg_err)?;
        Ok(detail)
    }

    async fn list_source_daily_digests(
        &self,
        user_id: Uuid,
        source_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<SourceDailyDigestView>, CoreError> {
        let sub_row = sqlx::query(
            r#"
            SELECT s.canonical_source_id
            FROM user_subscriptions s
            INNER JOIN canonical_sources c ON c.id = s.canonical_source_id
            WHERE s.user_id = $1
              AND (s.canonical_source_id = $2 OR s.id = $2)
              AND (c.scope = 'shared_public' OR c.scope_owner_id = $1)
            "#,
        )
        .bind(user_id)
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_pg_err)?
        .ok_or_else(|| CoreError::NotFound("source subscription not found".to_owned()))?;

        let canonical_source_id: Uuid = sub_row
            .try_get("canonical_source_id")
            .map_err(|e| col_err("canonical_source_id", e))?;

        let total_i64: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM source_daily_digests WHERE user_id = $1 AND canonical_source_id = $2",
        )
        .bind(user_id)
        .bind(canonical_source_id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;
        let total = u64::try_from(total_i64.max(0)).unwrap_or(0);

        let rows = sqlx::query(
            r#"
            SELECT
                sd.id,
                sd.digest_id,
                sd.canonical_source_id AS source_id,
                COALESCE(s.custom_name, c.default_name) AS source_name,
                sd.digest_date,
                sd.input_fingerprint,
                sd.summary_markdown,
                sd.key_points_json,
                sd.item_count,
                sd.created_at
            FROM source_daily_digests sd
            INNER JOIN canonical_sources c ON c.id = sd.canonical_source_id
            LEFT JOIN user_subscriptions s
                ON s.canonical_source_id = c.id
               AND s.user_id = $1
            WHERE sd.user_id = $1
              AND sd.canonical_source_id = $2
            ORDER BY sd.digest_date DESC, sd.created_at DESC
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind(user_id)
        .bind(canonical_source_id)
        .bind(i64::try_from(page.limit()).unwrap_or(i64::MAX))
        .bind(i64::try_from(page.offset()).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        let items = rows
            .iter()
            .map(|row| {
                let key_points: Json<Vec<String>> = row
                    .try_get("key_points_json")
                    .map_err(|e| col_err("key_points_json", e))?;
                Ok(SourceDailyDigestView {
                    id: row.try_get("id").map_err(|e| col_err("id", e))?,
                    digest_id: row
                        .try_get("digest_id")
                        .map_err(|e| col_err("digest_id", e))?,
                    source_id: row
                        .try_get("source_id")
                        .map_err(|e| col_err("source_id", e))?,
                    source_name: row
                        .try_get("source_name")
                        .map_err(|e| col_err("source_name", e))?,
                    digest_date: row
                        .try_get("digest_date")
                        .map_err(|e| col_err("digest_date", e))?,
                    input_fingerprint: row
                        .try_get("input_fingerprint")
                        .map_err(|e| col_err("input_fingerprint", e))?,
                    summary_markdown: row
                        .try_get("summary_markdown")
                        .map_err(|e| col_err("summary_markdown", e))?,
                    key_points: key_points.0,
                    item_count: row
                        .try_get("item_count")
                        .map_err(|e| col_err("item_count", e))?,
                    created_at: row
                        .try_get("created_at")
                        .map_err(|e| col_err("created_at", e))?,
                })
            })
            .collect::<Result<Vec<_>, CoreError>>()?;

        Ok(PagedResult::new(items, page, total))
    }
}

#[async_trait]
impl RunRepo for PostgresRepoStore {
    async fn create_run(&self, cmd: CreateRunCmd) -> Result<DigestRunRecord, CoreError> {
        let row = sqlx::query(
            r#"
            INSERT INTO digest_runs (
                id, user_id, run_type, status, started_at,
                items_fetched, items_new, prompt_tokens, completion_tokens
            )
            VALUES ($1, $2, $3, $4, $5, 0, 0, 0, 0)
            RETURNING id, user_id, run_type, status, started_at, finished_at,
                      items_fetched, items_new, prompt_tokens, completion_tokens, error_summary
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(cmd.user_id)
        .bind(cmd.run_type.as_str())
        .bind(RunStatus::Running.as_str())
        .bind(cmd.started_at)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        parse_digest_run_row(&row)
    }

    async fn finish_run(&self, run_id: Uuid, update: FinishRunCmd) -> Result<(), CoreError> {
        let res = sqlx::query(
            r#"
            UPDATE digest_runs
            SET status = $2,
                finished_at = $3,
                items_fetched = $4,
                items_new = $5,
                prompt_tokens = $6,
                completion_tokens = $7,
                error_summary = $8
            WHERE id = $1
            "#,
        )
        .bind(run_id)
        .bind(update.status.as_str())
        .bind(update.finished_at)
        .bind(update.items_fetched)
        .bind(update.items_new)
        .bind(update.prompt_tokens)
        .bind(update.completion_tokens)
        .bind(update.error_summary.as_deref())
        .execute(&self.pool)
        .await
        .map_err(map_pg_err)?;

        if res.rows_affected() == 0 {
            return Err(CoreError::NotFound("digest run not found".to_owned()));
        }
        Ok(())
    }

    async fn list_runs_with_deliveries(
        &self,
        user_id: Uuid,
        page: PageRequest,
    ) -> Result<PagedResult<DigestRunView>, CoreError> {
        let total_i64: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM digest_runs WHERE user_id = $1")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await
                .map_err(map_pg_err)?;
        let total = u64::try_from(total_i64.max(0)).unwrap_or(0);

        let run_rows = sqlx::query(
            r#"
            SELECT id, user_id, run_type, status, started_at, finished_at,
                   items_fetched, items_new, prompt_tokens, completion_tokens, error_summary
            FROM digest_runs
            WHERE user_id = $1
            ORDER BY started_at DESC, id DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(user_id)
        .bind(i64::try_from(page.limit()).unwrap_or(i64::MAX))
        .bind(i64::try_from(page.offset()).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        let runs = run_rows
            .iter()
            .map(parse_digest_run_row)
            .collect::<Result<Vec<_>, _>>()?;

        if runs.is_empty() {
            return Ok(PagedResult::new(Vec::new(), page, total));
        }

        let run_ids: Vec<Uuid> = runs.iter().map(|r| r.id).collect();
        let delivery_rows = sqlx::query(
            r#"
            SELECT id, run_id, user_id, digest_id, channel_id, channel_type, status, error_message, sent_at
            FROM delivery_logs
            WHERE user_id = $1 AND run_id = ANY($2)
            ORDER BY sent_at ASC, id ASC
            "#,
        )
        .bind(user_id)
        .bind(&run_ids)
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        let mut grouped: HashMap<Uuid, Vec<DeliveryLogRecord>> = HashMap::new();
        for row in &delivery_rows {
            let log = parse_delivery_log_row(row)?;
            grouped.entry(log.run_id).or_default().push(log);
        }

        let views = runs
            .into_iter()
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
                error_summary: r.error_summary,
                deliveries: grouped.remove(&r.id).unwrap_or_default(),
            })
            .collect();

        Ok(PagedResult::new(views, page, total))
    }

    async fn record_delivery(
        &self,
        cmd: CreateDeliveryLogCmd,
    ) -> Result<DeliveryLogRecord, CoreError> {
        let row = sqlx::query(
            r#"
            INSERT INTO delivery_logs (
                id, run_id, user_id, digest_id, channel_id,
                channel_type, status, error_message, sent_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING id, run_id, user_id, digest_id, channel_id, channel_type, status, error_message, sent_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(cmd.run_id)
        .bind(cmd.user_id)
        .bind(cmd.digest_id)
        .bind(cmd.channel_id)
        .bind(cmd.channel_type.as_str())
        .bind(cmd.status.as_str())
        .bind(cmd.error_message.as_deref())
        .bind(cmd.sent_at)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        parse_delivery_log_row(&row)
    }

    async fn get_delivery_by_id(
        &self,
        user_id: Uuid,
        delivery_id: Uuid,
    ) -> Result<Option<DeliveryLogRecord>, CoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, run_id, user_id, digest_id, channel_id, channel_type, status, error_message, sent_at
            FROM delivery_logs
            WHERE id = $1 AND user_id = $2
            "#,
        )
        .bind(delivery_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_pg_err)?;

        row.as_ref().map(parse_delivery_log_row).transpose()
    }

    async fn update_delivery_status(
        &self,
        user_id: Uuid,
        delivery_id: Uuid,
        status: DeliveryStatus,
        error_message: Option<String>,
        sent_at: DateTime<Utc>,
    ) -> Result<DeliveryLogRecord, CoreError> {
        let row = sqlx::query(
            r#"
            UPDATE delivery_logs
            SET status = $3,
                error_message = $4,
                sent_at = $5
            WHERE id = $1 AND user_id = $2
            RETURNING id, run_id, user_id, digest_id, channel_id, channel_type, status, error_message, sent_at
            "#,
        )
        .bind(delivery_id)
        .bind(user_id)
        .bind(status.as_str())
        .bind(error_message.as_deref())
        .bind(sent_at)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_pg_err)?
        .ok_or_else(|| CoreError::NotFound("delivery log not found".to_owned()))?;

        parse_delivery_log_row(&row)
    }

    async fn get_run_stats(&self, user_id: Uuid) -> Result<RunStatsView, CoreError> {
        let run_agg = sqlx::query(
            r#"
            SELECT
                COUNT(*)::BIGINT AS total_runs,
                COUNT(*) FILTER (WHERE status = $2)::BIGINT AS succeeded_runs,
                COUNT(*) FILTER (WHERE status = $3)::BIGINT AS failed_runs,
                COALESCE(SUM(prompt_tokens), 0)::BIGINT AS total_prompt_tokens,
                COALESCE(SUM(completion_tokens), 0)::BIGINT AS total_completion_tokens
            FROM digest_runs
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .bind(RunStatus::Succeeded.as_str())
        .bind(RunStatus::Failed.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        let delivery_agg = sqlx::query(
            r#"
            SELECT
                COUNT(*) FILTER (WHERE status = $2)::BIGINT AS deliveries_sent,
                COUNT(*) FILTER (WHERE status = $3)::BIGINT AS deliveries_failed
            FROM delivery_logs
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .bind(DeliveryStatus::Sent.as_str())
        .bind(DeliveryStatus::Failed.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        let health_rows = sqlx::query(
            r#"
            SELECT
                c.id AS source_id,
                s.custom_name AS source_name,
                c.source_type,
                c.scope,
                s.enabled,
                c.last_status,
                c.last_fetched_at,
                c.consecutive_failures,
                c.last_error
            FROM user_subscriptions s
            INNER JOIN canonical_sources c ON c.id = s.canonical_source_id
            WHERE s.user_id = $1
              AND (c.scope = 'shared_public' OR c.scope_owner_id = $1)
            ORDER BY s.created_at ASC, s.id ASC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        let source_health = health_rows
            .iter()
            .map(|row| {
                let source_type_raw: String = row
                    .try_get("source_type")
                    .map_err(|e| col_err("source_type", e))?;
                let scope_raw: String = row.try_get("scope").map_err(|e| col_err("scope", e))?;
                let status_raw: Option<String> = row
                    .try_get("last_status")
                    .map_err(|e| col_err("last_status", e))?;
                Ok(SourceHealthStat {
                    source_id: row
                        .try_get("source_id")
                        .map_err(|e| col_err("source_id", e))?,
                    source_name: row
                        .try_get("source_name")
                        .map_err(|e| col_err("source_name", e))?,
                    source_type: source_type_raw.parse::<SourceType>()?,
                    scope: scope_raw.parse::<SourceScope>()?,
                    enabled: row.try_get("enabled").map_err(|e| col_err("enabled", e))?,
                    last_status: status_raw
                        .as_deref()
                        .map(str::parse::<FetchOutcome>)
                        .transpose()?,
                    last_fetched_at: row
                        .try_get("last_fetched_at")
                        .map_err(|e| col_err("last_fetched_at", e))?,
                    consecutive_failures: row
                        .try_get("consecutive_failures")
                        .map_err(|e| col_err("consecutive_failures", e))?,
                    last_error: row
                        .try_get("last_error")
                        .map_err(|e| col_err("last_error", e))?,
                })
            })
            .collect::<Result<Vec<_>, CoreError>>()?;

        Ok(RunStatsView {
            total_runs: run_agg
                .try_get("total_runs")
                .map_err(|e| col_err("total_runs", e))?,
            succeeded_runs: run_agg
                .try_get("succeeded_runs")
                .map_err(|e| col_err("succeeded_runs", e))?,
            failed_runs: run_agg
                .try_get("failed_runs")
                .map_err(|e| col_err("failed_runs", e))?,
            total_prompt_tokens: run_agg
                .try_get("total_prompt_tokens")
                .map_err(|e| col_err("total_prompt_tokens", e))?,
            total_completion_tokens: run_agg
                .try_get("total_completion_tokens")
                .map_err(|e| col_err("total_completion_tokens", e))?,
            deliveries_sent: delivery_agg
                .try_get("deliveries_sent")
                .map_err(|e| col_err("deliveries_sent", e))?,
            deliveries_failed: delivery_agg
                .try_get("deliveries_failed")
                .map_err(|e| col_err("deliveries_failed", e))?,
            source_health,
        })
    }
}

#[async_trait]
impl SettingsRepo for PostgresRepoStore {
    async fn get_ai_config(&self, user_id: Uuid) -> Result<Option<UserAiConfigRecord>, CoreError> {
        let row = sqlx::query(
            r#"
            SELECT user_id, provider_label, base_url, api_key_enc, api_key_hint,
                   model, system_prompt, temperature, max_tokens, updated_at
            FROM user_ai_configs
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_pg_err)?;

        row.as_ref()
            .map(|r| {
                Ok(UserAiConfigRecord {
                    user_id: r.try_get("user_id").map_err(|e| col_err("user_id", e))?,
                    provider_label: r
                        .try_get("provider_label")
                        .map_err(|e| col_err("provider_label", e))?,
                    base_url: r.try_get("base_url").map_err(|e| col_err("base_url", e))?,
                    api_key_enc: r
                        .try_get("api_key_enc")
                        .map_err(|e| col_err("api_key_enc", e))?,
                    api_key_hint: r
                        .try_get("api_key_hint")
                        .map_err(|e| col_err("api_key_hint", e))?,
                    model: r.try_get("model").map_err(|e| col_err("model", e))?,
                    system_prompt: r
                        .try_get("system_prompt")
                        .map_err(|e| col_err("system_prompt", e))?,
                    temperature: r
                        .try_get("temperature")
                        .map_err(|e| col_err("temperature", e))?,
                    max_tokens: r
                        .try_get("max_tokens")
                        .map_err(|e| col_err("max_tokens", e))?,
                    updated_at: r
                        .try_get("updated_at")
                        .map_err(|e| col_err("updated_at", e))?,
                })
            })
            .transpose()
    }

    async fn upsert_ai_config(
        &self,
        user_id: Uuid,
        cmd: UpsertAiConfigRecord,
    ) -> Result<UserAiConfigRecord, CoreError> {
        let row = sqlx::query(
            r#"
            INSERT INTO user_ai_configs (
                user_id, provider_label, base_url, api_key_enc, api_key_hint,
                model, system_prompt, temperature, max_tokens, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            ON CONFLICT (user_id) DO UPDATE SET
                provider_label = EXCLUDED.provider_label,
                base_url = EXCLUDED.base_url,
                api_key_enc = EXCLUDED.api_key_enc,
                api_key_hint = EXCLUDED.api_key_hint,
                model = EXCLUDED.model,
                system_prompt = EXCLUDED.system_prompt,
                temperature = EXCLUDED.temperature,
                max_tokens = EXCLUDED.max_tokens,
                updated_at = NOW()
            RETURNING user_id, provider_label, base_url, api_key_enc, api_key_hint,
                      model, system_prompt, temperature, max_tokens, updated_at
            "#,
        )
        .bind(user_id)
        .bind(&cmd.provider_label)
        .bind(&cmd.base_url)
        .bind(cmd.api_key_enc.as_deref())
        .bind(cmd.api_key_hint.as_deref())
        .bind(&cmd.model)
        .bind(&cmd.system_prompt)
        .bind(cmd.temperature)
        .bind(cmd.max_tokens)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        Ok(UserAiConfigRecord {
            user_id: row.try_get("user_id").map_err(|e| col_err("user_id", e))?,
            provider_label: row
                .try_get("provider_label")
                .map_err(|e| col_err("provider_label", e))?,
            base_url: row
                .try_get("base_url")
                .map_err(|e| col_err("base_url", e))?,
            api_key_enc: row
                .try_get("api_key_enc")
                .map_err(|e| col_err("api_key_enc", e))?,
            api_key_hint: row
                .try_get("api_key_hint")
                .map_err(|e| col_err("api_key_hint", e))?,
            model: row.try_get("model").map_err(|e| col_err("model", e))?,
            system_prompt: row
                .try_get("system_prompt")
                .map_err(|e| col_err("system_prompt", e))?,
            temperature: row
                .try_get("temperature")
                .map_err(|e| col_err("temperature", e))?,
            max_tokens: row
                .try_get("max_tokens")
                .map_err(|e| col_err("max_tokens", e))?,
            updated_at: row
                .try_get("updated_at")
                .map_err(|e| col_err("updated_at", e))?,
        })
    }

    async fn list_channels(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<NotificationChannelRecord>, CoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, user_id, channel_type, name, enabled, config_enc, config_hint,
                   trigger_on_daily, trigger_on_alert, created_at
            FROM user_notification_channels
            WHERE user_id = $1
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        rows.iter().map(parse_channel_row).collect()
    }

    async fn get_channel(
        &self,
        user_id: Uuid,
        channel_id: Uuid,
    ) -> Result<Option<NotificationChannelRecord>, CoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, user_id, channel_type, name, enabled, config_enc, config_hint,
                   trigger_on_daily, trigger_on_alert, created_at
            FROM user_notification_channels
            WHERE id = $1 AND user_id = $2
            "#,
        )
        .bind(channel_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_pg_err)?;

        row.as_ref().map(parse_channel_row).transpose()
    }

    async fn create_channel(
        &self,
        user_id: Uuid,
        cmd: CreateChannelRecord,
    ) -> Result<NotificationChannelRecord, CoreError> {
        let row = sqlx::query(
            r#"
            INSERT INTO user_notification_channels (
                id, user_id, channel_type, name, enabled, config_enc, config_hint,
                trigger_on_daily, trigger_on_alert, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            RETURNING id, user_id, channel_type, name, enabled, config_enc, config_hint,
                      trigger_on_daily, trigger_on_alert, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(cmd.channel_type.as_str())
        .bind(&cmd.name)
        .bind(cmd.enabled)
        .bind(&cmd.config_enc)
        .bind(&cmd.config_hint)
        .bind(cmd.trigger_on_daily)
        .bind(cmd.trigger_on_alert)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        parse_channel_row(&row)
    }

    async fn update_channel(
        &self,
        user_id: Uuid,
        channel_id: Uuid,
        cmd: UpdateChannelRecord,
    ) -> Result<NotificationChannelRecord, CoreError> {
        let row = sqlx::query(
            r#"
            UPDATE user_notification_channels
            SET name = COALESCE($3, name),
                enabled = COALESCE($4, enabled),
                config_enc = COALESCE($5, config_enc),
                config_hint = COALESCE($6, config_hint),
                trigger_on_daily = COALESCE($7, trigger_on_daily),
                trigger_on_alert = COALESCE($8, trigger_on_alert)
            WHERE id = $1 AND user_id = $2
            RETURNING id, user_id, channel_type, name, enabled, config_enc, config_hint,
                      trigger_on_daily, trigger_on_alert, created_at
            "#,
        )
        .bind(channel_id)
        .bind(user_id)
        .bind(cmd.name.as_deref())
        .bind(cmd.enabled)
        .bind(cmd.config_enc.as_deref())
        .bind(cmd.config_hint.as_deref())
        .bind(cmd.trigger_on_daily)
        .bind(cmd.trigger_on_alert)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_pg_err)?
        .ok_or_else(|| CoreError::NotFound("notification channel not found".to_owned()))?;

        parse_channel_row(&row)
    }

    async fn delete_channel(&self, user_id: Uuid, channel_id: Uuid) -> Result<(), CoreError> {
        let res =
            sqlx::query("DELETE FROM user_notification_channels WHERE id = $1 AND user_id = $2")
                .bind(channel_id)
                .bind(user_id)
                .execute(&self.pool)
                .await
                .map_err(map_pg_err)?;

        if res.rows_affected() == 0 {
            return Err(CoreError::NotFound(
                "notification channel not found".to_owned(),
            ));
        }
        Ok(())
    }

    async fn get_schedule(&self, user_id: Uuid) -> Result<UserScheduleRecord, CoreError> {
        let default_interval = i32::try_from(DEFAULT_ALERT_INTERVAL_MINUTES).unwrap_or(60);
        sqlx::query(
            r#"
            INSERT INTO user_schedules (
                user_id, timezone, daily_time, email_enabled,
                send_when_empty, alert_scan_enabled, alert_interval_minutes
            )
            VALUES ($1, $2, $3, TRUE, FALSE, TRUE, $4)
            ON CONFLICT (user_id) DO NOTHING
            "#,
        )
        .bind(user_id)
        .bind(DEFAULT_TIMEZONE)
        .bind(DEFAULT_DAILY_TIME)
        .bind(default_interval)
        .execute(&self.pool)
        .await
        .map_err(map_pg_err)?;

        let row = sqlx::query(
            r#"
            SELECT user_id, timezone, daily_time, email_enabled, send_when_empty,
                   alert_scan_enabled, alert_interval_minutes, last_daily_run_date, last_alert_scan_at
            FROM user_schedules
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        parse_schedule_row(&row)
    }

    async fn upsert_schedule(
        &self,
        user_id: Uuid,
        cmd: UpdateScheduleCmd,
    ) -> Result<UserScheduleRecord, CoreError> {
        cmd.validate()?;
        let row = sqlx::query(
            r#"
            INSERT INTO user_schedules (
                user_id, timezone, daily_time, email_enabled,
                send_when_empty, alert_scan_enabled, alert_interval_minutes
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (user_id) DO UPDATE SET
                timezone = EXCLUDED.timezone,
                daily_time = EXCLUDED.daily_time,
                email_enabled = EXCLUDED.email_enabled,
                send_when_empty = EXCLUDED.send_when_empty,
                alert_scan_enabled = EXCLUDED.alert_scan_enabled,
                alert_interval_minutes = EXCLUDED.alert_interval_minutes
            RETURNING user_id, timezone, daily_time, email_enabled, send_when_empty,
                      alert_scan_enabled, alert_interval_minutes, last_daily_run_date, last_alert_scan_at
            "#,
        )
        .bind(user_id)
        .bind(cmd.timezone.trim())
        .bind(cmd.daily_time.trim())
        .bind(cmd.email_enabled)
        .bind(cmd.send_when_empty)
        .bind(cmd.alert_scan_enabled)
        .bind(cmd.alert_interval_minutes)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        parse_schedule_row(&row)
    }

    async fn list_due_daily_schedules(&self) -> Result<Vec<UserScheduleRecord>, CoreError> {
        let rows = sqlx::query(
            r#"
            SELECT user_id, timezone, daily_time, email_enabled, send_when_empty,
                   alert_scan_enabled, alert_interval_minutes, last_daily_run_date, last_alert_scan_at
            FROM user_schedules
            ORDER BY user_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        rows.iter().map(parse_schedule_row).collect()
    }

    async fn mark_daily_run_date(
        &self,
        user_id: Uuid,
        digest_date: NaiveDate,
    ) -> Result<(), CoreError> {
        let res =
            sqlx::query("UPDATE user_schedules SET last_daily_run_date = $2 WHERE user_id = $1")
                .bind(user_id)
                .bind(digest_date)
                .execute(&self.pool)
                .await
                .map_err(map_pg_err)?;

        if res.rows_affected() == 0 {
            return Err(CoreError::NotFound("user schedule not found".to_owned()));
        }
        Ok(())
    }

    async fn list_due_alert_schedules(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<UserScheduleRecord>, CoreError> {
        let rows = sqlx::query(
            r#"
            SELECT user_id, timezone, daily_time, email_enabled, send_when_empty,
                   alert_scan_enabled, alert_interval_minutes, last_daily_run_date, last_alert_scan_at
            FROM user_schedules
            WHERE alert_scan_enabled = TRUE
              AND (
                  last_alert_scan_at IS NULL
                  OR last_alert_scan_at + (GREATEST(alert_interval_minutes, 1) * INTERVAL '1 minute') <= $1
              )
            ORDER BY user_id ASC
            "#,
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        rows.iter().map(parse_schedule_row).collect()
    }

    async fn mark_alert_scan_at(
        &self,
        user_id: Uuid,
        scanned_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let res =
            sqlx::query("UPDATE user_schedules SET last_alert_scan_at = $2 WHERE user_id = $1")
                .bind(user_id)
                .bind(scanned_at)
                .execute(&self.pool)
                .await
                .map_err(map_pg_err)?;

        if res.rows_affected() == 0 {
            return Err(CoreError::NotFound("user schedule not found".to_owned()));
        }
        Ok(())
    }

    async fn list_watchlist_rules(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<WatchlistRuleRecord>, CoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, user_id, name, keywords, exclude_keywords, priority,
                   notify_immediately, enabled, created_at
            FROM user_watchlist_rules
            WHERE user_id = $1
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_pg_err)?;

        rows.iter().map(parse_watchlist_rule_row).collect()
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

        let row = sqlx::query(
            r#"
            INSERT INTO user_watchlist_rules (
                id, user_id, name, keywords, exclude_keywords, priority,
                notify_immediately, enabled, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
            RETURNING id, user_id, name, keywords, exclude_keywords, priority,
                      notify_immediately, enabled, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(cmd.name.trim())
        .bind(&cmd.keywords)
        .bind(&cmd.exclude_keywords)
        .bind(cmd.priority.as_str())
        .bind(cmd.notify_immediately)
        .bind(cmd.enabled)
        .fetch_one(&self.pool)
        .await
        .map_err(map_pg_err)?;

        parse_watchlist_rule_row(&row)
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

        let row = sqlx::query(
            r#"
            UPDATE user_watchlist_rules
            SET name = $3,
                keywords = $4,
                exclude_keywords = $5,
                priority = $6,
                notify_immediately = $7,
                enabled = $8
            WHERE id = $1 AND user_id = $2
            RETURNING id, user_id, name, keywords, exclude_keywords, priority,
                      notify_immediately, enabled, created_at
            "#,
        )
        .bind(rule_id)
        .bind(user_id)
        .bind(cmd.name.trim())
        .bind(&cmd.keywords)
        .bind(&cmd.exclude_keywords)
        .bind(cmd.priority.as_str())
        .bind(cmd.notify_immediately)
        .bind(cmd.enabled)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_pg_err)?
        .ok_or_else(|| CoreError::NotFound("watchlist rule not found".to_owned()))?;

        parse_watchlist_rule_row(&row)
    }

    async fn delete_watchlist_rule(&self, user_id: Uuid, rule_id: Uuid) -> Result<(), CoreError> {
        let res = sqlx::query("DELETE FROM user_watchlist_rules WHERE id = $1 AND user_id = $2")
            .bind(rule_id)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(map_pg_err)?;

        if res.rows_affected() == 0 {
            return Err(CoreError::NotFound("watchlist rule not found".to_owned()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIGRATION_0001_SQL: &str = include_str!("../../migrations/0001_init.sql");

    #[test]
    fn migration_0001_contains_all_14_tables_and_pg15_to_18_constraints() {
        let expected_tables = [
            "users",
            "user_ai_configs",
            "canonical_sources",
            "user_subscriptions",
            "canonical_items",
            "user_item_matches",
            "daily_digests",
            "source_daily_digests",
            "user_notification_channels",
            "user_schedules",
            "user_watchlist_rules",
            "digest_runs",
            "delivery_logs",
            "user_bookmarks",
        ];

        for table in expected_tables {
            let needle = format!("CREATE TABLE IF NOT EXISTS {table}");
            assert!(
                MIGRATION_0001_SQL.contains(&needle),
                "missing table DDL: {needle}"
            );
        }
        assert_eq!(
            MIGRATION_0001_SQL
                .matches("CREATE TABLE IF NOT EXISTS ")
                .count(),
            14
        );

        assert!(MIGRATION_0001_SQL
            .contains("UNIQUE NULLS NOT DISTINCT (source_type, canonical_key, scope_owner_id)"));
        assert!(MIGRATION_0001_SQL
            .contains("CHECK (\n        (scope = 'shared_public' AND scope_owner_id IS NULL)"));
        assert!(MIGRATION_0001_SQL.contains("UNIQUE (canonical_source_id, canonical_url)"));
        assert!(MIGRATION_0001_SQL.contains("UNIQUE (canonical_source_id, content_hash)"));
        assert!(MIGRATION_0001_SQL.contains("search_tsv TSVECTOR"));
        assert!(
            MIGRATION_0001_SQL.contains("BEFORE INSERT OR UPDATE OF title, author, summary_raw")
        );
        assert!(MIGRATION_0001_SQL.contains("setweight(to_tsvector('simple'"));
        assert!(MIGRATION_0001_SQL.contains("USING GIN (search_tsv)"));
    }

    #[tokio::test]
    async fn postgres_repo_store_builds_repositories_and_masks_without_cipher() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:test@127.0.0.1:5432/digest_test")
            .unwrap();
        let store = PostgresRepositories::new(pool.clone());
        let rec = UserSourceRecord {
            subscription_id: Uuid::new_v4(),
            canonical_source_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            source_type: SourceType::Rss,
            scope: SourceScope::SharedPublic,
            scope_owner_id: None,
            canonical_key: "google-ai-blog".to_owned(),
            default_name: "Google AI Blog".to_owned(),
            custom_name: "Google AI".to_owned(),
            category: "Official".to_owned(),
            enabled: true,
            config_json_enc: Some("v1:ciphertext".to_owned()),
            etag: None,
            last_modified: None,
            last_fetched_at: None,
            next_poll_at: None,
            last_status: None,
            last_error: None,
            consecutive_failures: 0,
            created_at: Utc::now(),
        };

        let view = store.record_to_source_view(&rec).unwrap();
        assert!(view.handle_or_url.starts_with("https://"));
        assert!(view.has_credentials);
        assert_eq!(
            view.config_hint.as_deref(),
            Some(SECRET_MASK_CONFIGURED_PLACEHOLDER)
        );

        let cipher = Arc::new(EnvelopeCipher::from_bytes(
            [0x19u8; digest_core::constants::CRYPTO_KEY_BYTE_LEN],
        ));
        let enc = crate::repo::dto::encode_source_config(
            &cipher,
            "https://example.com/custom.xml",
            Some(serde_json::json!({ "bearerToken": "test-token-abcd-1234" })),
        )
        .unwrap();
        let store_with_cipher = PostgresRepoStore::with_cipher(pool.clone(), Arc::clone(&cipher));
        let rec_with_cipher = UserSourceRecord {
            config_json_enc: Some(enc),
            canonical_key: "custom-rss".to_owned(),
            ..rec
        };
        let decoded_view = store_with_cipher
            .record_to_source_view(&rec_with_cipher)
            .unwrap();
        assert_eq!(decoded_view.handle_or_url, "https://example.com/custom.xml");
        assert!(decoded_view.has_credentials);
        assert_eq!(decoded_view.config_hint.as_deref(), Some("test****1234"));

        let _repos = repositories(pool.clone());
        let _repos_with_cipher = repositories_with_cipher(pool, cipher);
    }
}
