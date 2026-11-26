//! HTTP REST and SSE router tree wiring all Section 5 endpoints.

pub mod admin;
pub mod bookmarks;
pub mod digests;
pub mod error;
pub mod events;
pub mod items;
pub mod me;
pub mod runs;
pub mod settings;
pub mod sources;

use axum::routing::{any, delete, get, post};
use axum::{Json, Router};
use digest_core::constants::{
    API_PREFIX, HEALTH_STATUS_OK, ROUTE_API_ROOT, ROUTE_HEALTH, SERVICE_NAME, SPA_INDEX_FILE,
    SUBROUTE_API_V1,
};
use digest_core::error::CoreError;
use serde::Serialize;
use tower_http::services::{ServeDir, ServeFile};

use crate::api::error::{ApiError, RequestId};

use crate::constants::{
    SUBROUTE_ADMIN_PLATFORM, SUBROUTE_ADMIN_PLATFORM_X_TEST, SUBROUTE_AUTH_CONFIG,
    SUBROUTE_BOOKMARKS, SUBROUTE_BOOKMARK_BY_ITEM, SUBROUTE_DELIVERY_RESEND, SUBROUTE_DIGESTS,
    SUBROUTE_DIGEST_BY_DATE, SUBROUTE_DIGEST_MARKDOWN, SUBROUTE_EVENTS_STREAM, SUBROUTE_ITEMS,
    SUBROUTE_ME, SUBROUTE_ME_PREFERENCES, SUBROUTE_RUNS, SUBROUTE_RUNS_STATS,
    SUBROUTE_RUNS_TRIGGER, SUBROUTE_SETTINGS_AI, SUBROUTE_SETTINGS_AI_MODELS,
    SUBROUTE_SETTINGS_AI_TEST, SUBROUTE_SETTINGS_CHANNELS, SUBROUTE_SETTINGS_CHANNEL_BY_ID,
    SUBROUTE_SETTINGS_CHANNEL_TEST, SUBROUTE_SETTINGS_SCHEDULE, SUBROUTE_SETTINGS_SOURCES,
    SUBROUTE_SETTINGS_SOURCES_EXPORT, SUBROUTE_SETTINGS_SOURCES_IMPORT,
    SUBROUTE_SETTINGS_SOURCES_IMPORT_PRESET, SUBROUTE_SETTINGS_SOURCE_BY_ID,
    SUBROUTE_SETTINGS_SOURCE_TEST, SUBROUTE_SETTINGS_WATCHLIST, SUBROUTE_SETTINGS_WATCHLIST_BY_ID,
    SUBROUTE_SOURCE_DIGESTS, SUBROUTE_SOURCE_ITEMS,
};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
}

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: HEALTH_STATUS_OK,
        service: SERVICE_NAME,
    })
}

async fn api_not_found(req_id: RequestId) -> ApiError {
    ApiError::new(
        req_id.0,
        CoreError::NotFound("API endpoint not found".to_owned()),
    )
}

/// Mounts all Section 5 REST and SSE routes onto the Axum router with shared `AppState`.
pub fn router(state: AppState) -> Router {
    let static_dir = state.config.core.static_dir.clone();

    let api_v1 = Router::new()
        .route(SUBROUTE_AUTH_CONFIG, get(me::auth_config))
        .route(SUBROUTE_ME, get(me::get_me))
        .route(
            SUBROUTE_ME_PREFERENCES,
            axum::routing::put(me::update_me_preferences),
        )
        .route(SUBROUTE_DIGESTS, get(digests::list_digests))
        .route(SUBROUTE_DIGEST_BY_DATE, get(digests::get_digest_by_date))
        .route(SUBROUTE_DIGEST_MARKDOWN, get(digests::get_digest_markdown))
        .route(SUBROUTE_SOURCE_DIGESTS, get(sources::list_source_digests))
        .route(SUBROUTE_SOURCE_ITEMS, get(sources::list_source_items))
        .route(SUBROUTE_ITEMS, get(items::search_items))
        .route(SUBROUTE_BOOKMARKS, post(bookmarks::upsert_bookmark))
        .route(
            SUBROUTE_BOOKMARK_BY_ITEM,
            delete(bookmarks::delete_bookmark),
        )
        .route(
            SUBROUTE_SETTINGS_AI,
            get(settings::get_ai_config).put(settings::update_ai_config),
        )
        .route(SUBROUTE_SETTINGS_AI_TEST, post(settings::test_ai_config))
        .route(SUBROUTE_SETTINGS_AI_MODELS, post(settings::list_ai_models))
        .route(
            SUBROUTE_SETTINGS_SOURCES,
            get(settings::list_sources).post(settings::create_source),
        )
        .route(
            SUBROUTE_SETTINGS_SOURCE_BY_ID,
            axum::routing::put(settings::update_source).delete(settings::delete_source),
        )
        .route(
            SUBROUTE_SETTINGS_SOURCES_IMPORT_PRESET,
            post(settings::import_preset_sources),
        )
        .route(
            SUBROUTE_SETTINGS_SOURCES_EXPORT,
            post(settings::export_sources),
        )
        .route(
            SUBROUTE_SETTINGS_SOURCES_IMPORT,
            post(settings::import_sources),
        )
        .route(SUBROUTE_SETTINGS_SOURCE_TEST, post(settings::test_source))
        .route(
            SUBROUTE_SETTINGS_CHANNELS,
            get(settings::list_channels).post(settings::create_channel),
        )
        .route(
            SUBROUTE_SETTINGS_CHANNEL_BY_ID,
            axum::routing::put(settings::update_channel).delete(settings::delete_channel),
        )
        .route(SUBROUTE_SETTINGS_CHANNEL_TEST, post(settings::test_channel))
        .route(
            SUBROUTE_SETTINGS_SCHEDULE,
            get(settings::get_schedule).put(settings::update_schedule),
        )
        .route(
            SUBROUTE_SETTINGS_WATCHLIST,
            get(settings::list_watchlist).post(settings::create_watchlist),
        )
        .route(
            SUBROUTE_SETTINGS_WATCHLIST_BY_ID,
            axum::routing::put(settings::update_watchlist).delete(settings::delete_watchlist),
        )
        .route(SUBROUTE_RUNS_TRIGGER, post(runs::trigger_run))
        .route(SUBROUTE_RUNS, get(runs::list_runs))
        .route(SUBROUTE_RUNS_STATS, get(runs::get_run_stats))
        .route(SUBROUTE_DELIVERY_RESEND, post(runs::resend_delivery))
        .route(SUBROUTE_EVENTS_STREAM, get(events::stream_events))
        .route(
            SUBROUTE_ADMIN_PLATFORM,
            get(admin::get_platform_settings).put(admin::update_platform_settings),
        )
        .route(SUBROUTE_ADMIN_PLATFORM_X_TEST, post(admin::test_platform_x))
        .route("/", any(api_not_found))
        .fallback(api_not_found);

    let api_root = Router::new()
        .route("/", any(api_not_found))
        .nest(SUBROUTE_API_V1, api_v1)
        .fallback(api_not_found);

    let api_trailing_slash = format!("{ROUTE_API_ROOT}/");
    let api_v1_trailing_slash = format!("{API_PREFIX}/");

    let base_router = Router::new()
        .route(ROUTE_HEALTH, get(health_handler))
        .route(&api_trailing_slash, any(api_not_found))
        .route(&api_v1_trailing_slash, any(api_not_found))
        .nest(ROUTE_API_ROOT, api_root)
        .with_state(state);

    match static_dir {
        Some(dir) => {
            let spa_index = dir.join(SPA_INDEX_FILE);
            base_router.fallback_service(ServeDir::new(&dir).fallback(ServeFile::new(spa_index)))
        }
        None => base_router,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use chrono::{DateTime, NaiveDate, Utc};
    use digest_core::cache::InMemoryCacheCoordinator;
    use digest_core::config::AppConfig;
    use digest_core::constants::CRYPTO_KEY_BYTE_LEN;
    use digest_core::crypto::EnvelopeCipher;
    use digest_core::domain::{
        AuthMode, DeliveryStatus, ErrorCode, FetchOutcome, NotificationChannelType, RunStatus,
        RunType, SourceScope, SseEventType,
    };
    use digest_core::error::CoreError;
    use digest_core::ssrf::SsrfGuard;
    use http::header::CONTENT_TYPE;
    use http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tokio::sync::{broadcast, Mutex};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::*;
    use crate::auth::HybridAuthService;
    use crate::config::ServerConfig;
    use crate::constants::{CONTENT_TYPE_MARKDOWN_UTF8, DEV_BYPASS_OIDC_SUB};
    use crate::repo::*;
    use crate::services::{
        AiModelsRequest, AiModelsResult, AiTestRequest, AiTestResult, ChannelTestResult,
        ExecutionOrchestrator, SourceTestResult, SseEnvelope, SsePublisher, TriggerRunRequest,
    };

    #[derive(Default)]
    struct MockStore {
        users: Mutex<HashMap<Uuid, UserRecord>>,
        ai_configs: Mutex<HashMap<Uuid, UserAiConfigRecord>>,
        sources: Mutex<Vec<UserSourceRecord>>,
        channels: Mutex<Vec<NotificationChannelRecord>>,
        schedules: Mutex<HashMap<Uuid, UserScheduleRecord>>,
        watchlist: Mutex<Vec<WatchlistRuleRecord>>,
        digests: Mutex<Vec<DailyDigestDetail>>,
        source_digests: Mutex<Vec<SourceDailyDigestView>>,
        items: Mutex<Vec<ItemView>>,
        runs: Mutex<Vec<DigestRunView>>,
        deliveries: Mutex<Vec<DeliveryLogRecord>>,
        platform_settings: Mutex<Option<PlatformSettingsRecord>>,
        cipher: Option<Arc<EnvelopeCipher>>,
    }

    impl MockStore {
        fn new(cipher: Arc<EnvelopeCipher>) -> Self {
            Self {
                cipher: Some(cipher),
                ..Self::default()
            }
        }
    }

    #[async_trait]
    impl UserRepo for MockStore {
        async fn upsert_oidc_user(&self, cmd: UpsertUserCmd) -> Result<UserRecord, CoreError> {
            let mut users = self.users.lock().await;
            if let Some(existing) = users.values().find(|u| u.oidc_sub == cmd.oidc_sub) {
                return Ok(existing.clone());
            }
            let user = UserRecord {
                id: Uuid::new_v4(),
                oidc_sub: cmd.oidc_sub,
                email: cmd.email,
                display_name: cmd.display_name,
                role: cmd.role,
                preferred_locale: digest_core::domain::default_digest_locale(),
                created_at: Utc::now(),
                last_login_at: Some(Utc::now()),
            };
            users.insert(user.id, user.clone());
            Ok(user)
        }

        async fn get_user_by_id(&self, user_id: Uuid) -> Result<Option<UserRecord>, CoreError> {
            Ok(self.users.lock().await.get(&user_id).cloned())
        }

        async fn update_user_locale(
            &self,
            user_id: Uuid,
            preferred_locale: &str,
        ) -> Result<UserRecord, CoreError> {
            let mut users = self.users.lock().await;
            let user = users
                .get_mut(&user_id)
                .ok_or_else(|| CoreError::NotFound("user not found".to_owned()))?;
            user.preferred_locale =
                digest_core::domain::normalize_digest_locale(preferred_locale).to_owned();
            Ok(user.clone())
        }

        async fn get_onboarding_flags(&self, user_id: Uuid) -> Result<OnboardingFlags, CoreError> {
            Ok(OnboardingFlags {
                has_ai_config: self.ai_configs.lock().await.contains_key(&user_id),
                has_sources: self
                    .sources
                    .lock()
                    .await
                    .iter()
                    .any(|s| s.user_id == user_id),
                has_channels: self
                    .channels
                    .lock()
                    .await
                    .iter()
                    .any(|c| c.user_id == user_id),
                has_schedule: self.schedules.lock().await.contains_key(&user_id),
            })
        }
    }

    #[async_trait]
    impl SourceRepo for MockStore {
        async fn create_or_subscribe(
            &self,
            user_id: Uuid,
            cmd: SubscribeSourceCmd,
        ) -> Result<UserSourceView, CoreError> {
            let mut sources = self.sources.lock().await;
            if let Some(existing) = sources.iter().find(|s| {
                s.user_id == user_id
                    && s.source_type == cmd.source_type
                    && s.canonical_key == cmd.canonical_key
            }) {
                return existing.to_view(self.cipher.as_ref().unwrap());
            }
            let rec = UserSourceRecord {
                subscription_id: Uuid::new_v4(),
                canonical_source_id: Uuid::new_v4(),
                user_id,
                source_type: cmd.source_type,
                scope: cmd.scope,
                scope_owner_id: cmd.scope_owner_id(user_id),
                canonical_key: cmd.canonical_key,
                default_name: cmd.default_name,
                custom_name: cmd.custom_name,
                category: cmd.category,
                enabled: cmd.enabled,
                config_json_enc: cmd.config_json_enc,
                etag: None,
                last_modified: None,
                last_fetched_at: None,
                next_poll_at: None,
                last_status: None,
                last_error: None,
                consecutive_failures: 0,
                created_at: Utc::now(),
            };
            let view = rec.to_view(self.cipher.as_ref().unwrap())?;
            sources.push(rec);
            Ok(view)
        }

        async fn list_user_sources(&self, user_id: Uuid) -> Result<Vec<UserSourceView>, CoreError> {
            let sources = self.sources.lock().await;
            let cipher = self.cipher.as_ref().unwrap();
            sources
                .iter()
                .filter(|s| s.user_id == user_id)
                .map(|s| s.to_view(cipher))
                .collect()
        }

        async fn get_user_source(
            &self,
            user_id: Uuid,
            id_or_sub: Uuid,
        ) -> Result<Option<UserSourceRecord>, CoreError> {
            Ok(self
                .sources
                .lock()
                .await
                .iter()
                .find(|s| {
                    s.user_id == user_id
                        && (s.canonical_source_id == id_or_sub || s.subscription_id == id_or_sub)
                })
                .cloned())
        }

        async fn update_user_subscription(
            &self,
            user_id: Uuid,
            id_or_sub: Uuid,
            cmd: UpdateSourceCmd,
        ) -> Result<UserSourceView, CoreError> {
            let mut sources = self.sources.lock().await;
            let rec = sources
                .iter_mut()
                .find(|s| {
                    s.user_id == user_id
                        && (s.canonical_source_id == id_or_sub || s.subscription_id == id_or_sub)
                })
                .ok_or_else(|| CoreError::NotFound("source not found".to_owned()))?;
            if let Some(name) = cmd.custom_name {
                rec.custom_name = name;
            }
            if let Some(cat) = cmd.category {
                rec.category = cat;
            }
            if let Some(en) = cmd.enabled {
                rec.enabled = en;
            }
            if let Some(enc) = cmd.config_json_enc {
                rec.config_json_enc = Some(enc);
            }
            rec.to_view(self.cipher.as_ref().unwrap())
        }

        async fn delete_user_subscription(
            &self,
            user_id: Uuid,
            id_or_sub: Uuid,
        ) -> Result<(), CoreError> {
            let mut sources = self.sources.lock().await;
            let before = sources.len();
            sources.retain(|s| {
                !(s.user_id == user_id
                    && (s.canonical_source_id == id_or_sub || s.subscription_id == id_or_sub))
            });
            if sources.len() == before {
                return Err(CoreError::NotFound("source not found".to_owned()));
            }
            Ok(())
        }

        async fn list_pollable_sources(
            &self,
            _: Option<SourceScope>,
            _: DateTime<Utc>,
            _: usize,
        ) -> Result<Vec<CanonicalSourceRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn record_fetch_outcome(
            &self,
            _: Uuid,
            _: SourceFetchStateUpdate,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn upsert_canonical_items(
            &self,
            _: Uuid,
            _: &[NewCanonicalItem],
        ) -> Result<Vec<CanonicalItemRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn list_recent_source_items(
            &self,
            _: Uuid,
            _: DateTime<Utc>,
            _: usize,
        ) -> Result<Vec<CanonicalItemRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn list_source_items(
            &self,
            _: Uuid,
            source_id: Uuid,
            page: PageRequest,
        ) -> Result<PagedResult<ItemView>, CoreError> {
            let items: Vec<ItemView> = self
                .items
                .lock()
                .await
                .iter()
                .filter(|i| i.source_id == source_id)
                .cloned()
                .collect();
            let total = items.len() as u64;
            Ok(PagedResult::new(items, page, total))
        }

        async fn search_user_items(
            &self,
            _: Uuid,
            query: ItemSearchQuery,
        ) -> Result<PagedResult<ItemView>, CoreError> {
            let items: Vec<ItemView> = self
                .items
                .lock()
                .await
                .iter()
                .filter(|i| query.source_id.is_none_or(|sid| i.source_id == sid))
                .cloned()
                .collect();
            let total = items.len() as u64;
            Ok(PagedResult::new(items, query.page, total))
        }

        async fn upsert_user_item_matches(
            &self,
            _: Uuid,
            _: &[NewUserItemMatch],
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn list_pending_urgent_alerts(
            &self,
            _: Uuid,
        ) -> Result<Vec<PendingAlertItem>, CoreError> {
            Ok(Vec::new())
        }

        async fn mark_items_alerted(
            &self,
            _: Uuid,
            _: &[Uuid],
            _: DateTime<Utc>,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn prune_unbookmarked_bodies(&self, _: DateTime<Utc>) -> Result<u64, CoreError> {
            Ok(0)
        }

        async fn upsert_bookmark(
            &self,
            _: Uuid,
            item_id: Uuid,
            note: Option<String>,
        ) -> Result<BookmarkView, CoreError> {
            Ok(BookmarkView {
                id: Uuid::new_v4(),
                item_id,
                note,
                created_at: Utc::now(),
            })
        }

        async fn delete_bookmark(&self, _: Uuid, _: Uuid) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[async_trait]
    impl DigestRepo for MockStore {
        async fn list_daily_digests(
            &self,
            _: Uuid,
            page: PageRequest,
        ) -> Result<PagedResult<DailyDigestSummary>, CoreError> {
            let summaries: Vec<DailyDigestSummary> = self
                .digests
                .lock()
                .await
                .iter()
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
            let total = summaries.len() as u64;
            Ok(PagedResult::new(summaries, page, total))
        }

        async fn get_daily_digest_by_date(
            &self,
            _: Uuid,
            date: NaiveDate,
        ) -> Result<Option<DailyDigestDetail>, CoreError> {
            Ok(self
                .digests
                .lock()
                .await
                .iter()
                .find(|d| d.digest_date == date)
                .cloned())
        }

        async fn find_daily_digest_by_fingerprint(
            &self,
            _: Uuid,
            _: NaiveDate,
            _: &str,
        ) -> Result<Option<DailyDigestDetail>, CoreError> {
            Ok(None)
        }

        async fn save_daily_and_source_digests(
            &self,
            _: SaveDigestCmd,
        ) -> Result<DailyDigestDetail, CoreError> {
            unreachable!()
        }

        async fn list_source_daily_digests(
            &self,
            _: Uuid,
            source_id: Uuid,
            page: PageRequest,
        ) -> Result<PagedResult<SourceDailyDigestView>, CoreError> {
            let list: Vec<SourceDailyDigestView> = self
                .source_digests
                .lock()
                .await
                .iter()
                .filter(|sd| sd.source_id == source_id)
                .cloned()
                .collect();
            let total = list.len() as u64;
            Ok(PagedResult::new(list, page, total))
        }
    }

    #[async_trait]
    impl RunRepo for MockStore {
        async fn create_run(&self, _: CreateRunCmd) -> Result<DigestRunRecord, CoreError> {
            unreachable!()
        }
        async fn finish_run(&self, _: Uuid, _: FinishRunCmd) -> Result<(), CoreError> {
            Ok(())
        }
        async fn list_runs_with_deliveries(
            &self,
            _: Uuid,
            page: PageRequest,
        ) -> Result<PagedResult<DigestRunView>, CoreError> {
            let runs = self.runs.lock().await.clone();
            let total = runs.len() as u64;
            Ok(PagedResult::new(runs, page, total))
        }
        async fn record_delivery(
            &self,
            _: CreateDeliveryLogCmd,
        ) -> Result<DeliveryLogRecord, CoreError> {
            unreachable!()
        }
        async fn get_delivery_by_id(
            &self,
            user_id: Uuid,
            delivery_id: Uuid,
        ) -> Result<Option<DeliveryLogRecord>, CoreError> {
            Ok(self
                .deliveries
                .lock()
                .await
                .iter()
                .find(|d| d.id == delivery_id && d.user_id == user_id)
                .cloned())
        }
        async fn update_delivery_status(
            &self,
            _: Uuid,
            _: Uuid,
            _: DeliveryStatus,
            _: Option<String>,
            _: DateTime<Utc>,
        ) -> Result<DeliveryLogRecord, CoreError> {
            unreachable!()
        }
        async fn get_run_stats(&self, _: Uuid) -> Result<RunStatsView, CoreError> {
            Ok(RunStatsView {
                total_runs: 1,
                succeeded_runs: 1,
                failed_runs: 0,
                total_prompt_tokens: 120,
                total_completion_tokens: 60,
                deliveries_sent: 1,
                deliveries_failed: 0,
                source_health: Vec::new(),
            })
        }
    }

    #[async_trait]
    impl SettingsRepo for MockStore {
        async fn get_ai_config(
            &self,
            user_id: Uuid,
        ) -> Result<Option<UserAiConfigRecord>, CoreError> {
            Ok(self.ai_configs.lock().await.get(&user_id).cloned())
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
            self.ai_configs.lock().await.insert(user_id, rec.clone());
            Ok(rec)
        }

        async fn list_channels(
            &self,
            user_id: Uuid,
        ) -> Result<Vec<NotificationChannelRecord>, CoreError> {
            Ok(self
                .channels
                .lock()
                .await
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
                .await
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
            self.channels.lock().await.push(rec.clone());
            Ok(rec)
        }

        async fn update_channel(
            &self,
            user_id: Uuid,
            channel_id: Uuid,
            cmd: UpdateChannelRecord,
        ) -> Result<NotificationChannelRecord, CoreError> {
            let mut channels = self.channels.lock().await;
            let rec = channels
                .iter_mut()
                .find(|c| c.user_id == user_id && c.id == channel_id)
                .ok_or_else(|| CoreError::NotFound("channel not found".to_owned()))?;
            if let Some(name) = cmd.name {
                rec.name = name;
            }
            if let Some(en) = cmd.enabled {
                rec.enabled = en;
            }
            if let Some(enc) = cmd.config_enc {
                rec.config_enc = enc;
            }
            if let Some(hint) = cmd.config_hint {
                rec.config_hint = hint;
            }
            if let Some(tod) = cmd.trigger_on_daily {
                rec.trigger_on_daily = tod;
            }
            if let Some(toa) = cmd.trigger_on_alert {
                rec.trigger_on_alert = toa;
            }
            Ok(rec.clone())
        }

        async fn delete_channel(&self, user_id: Uuid, channel_id: Uuid) -> Result<(), CoreError> {
            let mut channels = self.channels.lock().await;
            let before = channels.len();
            channels.retain(|c| !(c.user_id == user_id && c.id == channel_id));
            if channels.len() == before {
                return Err(CoreError::NotFound("channel not found".to_owned()));
            }
            Ok(())
        }

        async fn get_schedule(&self, user_id: Uuid) -> Result<UserScheduleRecord, CoreError> {
            let mut schedules = self.schedules.lock().await;
            let rec = schedules
                .entry(user_id)
                .or_insert_with(|| UserScheduleRecord {
                    user_id,
                    timezone: "Asia/Shanghai".to_owned(),
                    daily_time: "08:30".to_owned(),
                    email_enabled: true,
                    send_when_empty: false,
                    alert_scan_enabled: true,
                    alert_interval_minutes: 60,
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
            let rec = UserScheduleRecord {
                user_id,
                timezone: cmd.timezone,
                daily_time: cmd.daily_time,
                email_enabled: cmd.email_enabled,
                send_when_empty: cmd.send_when_empty,
                alert_scan_enabled: cmd.alert_scan_enabled,
                alert_interval_minutes: cmd.alert_interval_minutes,
                last_daily_run_date: None,
                last_alert_scan_at: None,
            };
            self.schedules.lock().await.insert(user_id, rec.clone());
            Ok(rec)
        }

        async fn list_due_daily_schedules(&self) -> Result<Vec<UserScheduleRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn mark_daily_run_date(&self, _: Uuid, _: NaiveDate) -> Result<(), CoreError> {
            Ok(())
        }

        async fn list_due_alert_schedules(
            &self,
            _: DateTime<Utc>,
        ) -> Result<Vec<UserScheduleRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn mark_alert_scan_at(&self, _: Uuid, _: DateTime<Utc>) -> Result<(), CoreError> {
            Ok(())
        }

        async fn list_watchlist_rules(
            &self,
            user_id: Uuid,
        ) -> Result<Vec<WatchlistRuleRecord>, CoreError> {
            Ok(self
                .watchlist
                .lock()
                .await
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
            self.watchlist.lock().await.push(rec.clone());
            Ok(rec)
        }

        async fn update_watchlist_rule(
            &self,
            user_id: Uuid,
            rule_id: Uuid,
            cmd: UpsertWatchlistRuleCmd,
        ) -> Result<WatchlistRuleRecord, CoreError> {
            let mut watchlist = self.watchlist.lock().await;
            let rec = watchlist
                .iter_mut()
                .find(|w| w.user_id == user_id && w.id == rule_id)
                .ok_or_else(|| CoreError::NotFound("watchlist rule not found".to_owned()))?;
            rec.name = cmd.name;
            rec.keywords = cmd.keywords;
            rec.exclude_keywords = cmd.exclude_keywords;
            rec.priority = cmd.priority;
            rec.notify_immediately = cmd.notify_immediately;
            rec.enabled = cmd.enabled;
            Ok(rec.clone())
        }

        async fn delete_watchlist_rule(
            &self,
            user_id: Uuid,
            rule_id: Uuid,
        ) -> Result<(), CoreError> {
            let mut watchlist = self.watchlist.lock().await;
            let before = watchlist.len();
            watchlist.retain(|w| !(w.user_id == user_id && w.id == rule_id));
            if watchlist.len() == before {
                return Err(CoreError::NotFound("watchlist rule not found".to_owned()));
            }
            Ok(())
        }

        async fn get_platform_settings(&self) -> Result<Option<PlatformSettingsRecord>, CoreError> {
            Ok(self.platform_settings.lock().await.clone())
        }

        async fn upsert_platform_settings(
            &self,
            cmd: UpsertPlatformSettingsRecord,
        ) -> Result<PlatformSettingsRecord, CoreError> {
            let rec = PlatformSettingsRecord {
                x_bridge_base_url: cmd.x_bridge_base_url,
                x_bridge_reload_url: cmd.x_bridge_reload_url,
                x_bridge_reload_secret_enc: cmd.x_bridge_reload_secret_enc,
                x_bridge_reload_secret_hint: cmd.x_bridge_reload_secret_hint,
                x_auth_token_enc: cmd.x_auth_token_enc,
                x_auth_token_hint: cmd.x_auth_token_hint,
                x_bearer_token_enc: cmd.x_bearer_token_enc,
                x_bearer_token_hint: cmd.x_bearer_token_hint,
                updated_by: cmd.updated_by,
                updated_at: Some(Utc::now()),
            };
            *self.platform_settings.lock().await = Some(rec.clone());
            Ok(rec)
        }
    }

    struct MockSse {
        txs: std::sync::Mutex<HashMap<Uuid, broadcast::Sender<SseEnvelope>>>,
    }

    impl MockSse {
        fn new() -> Self {
            Self {
                txs: std::sync::Mutex::new(HashMap::new()),
            }
        }
    }

    impl SsePublisher for MockSse {
        fn publish(&self, user_id: Uuid, event: SseEnvelope) {
            let mut guard = self.txs.lock().unwrap();
            let tx = guard
                .entry(user_id)
                .or_insert_with(|| broadcast::channel(16).0);
            let _ = tx.send(event);
        }

        fn subscribe(&self, user_id: Uuid) -> broadcast::Receiver<SseEnvelope> {
            let mut guard = self.txs.lock().unwrap();
            let tx = guard
                .entry(user_id)
                .or_insert_with(|| broadcast::channel(16).0);
            tx.subscribe()
        }
    }

    struct MockExecutor {
        store: Arc<MockStore>,
    }

    #[async_trait]
    impl ExecutionOrchestrator for MockExecutor {
        async fn trigger_manual_run(
            &self,
            _: Uuid,
            _: TriggerRunRequest,
        ) -> Result<DigestRunView, CoreError> {
            let run = DigestRunView {
                id: Uuid::new_v4(),
                run_type: RunType::ManualDaily,
                status: RunStatus::Succeeded,
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                items_fetched: 3,
                items_new: 2,
                prompt_tokens: 100,
                completion_tokens: 50,
                error_summary: None,
                deliveries: Vec::new(),
            };
            self.store.runs.lock().await.push(run.clone());
            Ok(run)
        }

        async fn test_ai_config(
            &self,
            _: Uuid,
            req: AiTestRequest,
        ) -> Result<AiTestResult, CoreError> {
            Ok(AiTestResult {
                ok: true,
                provider_label: req.provider_label.unwrap_or_else(|| "DeepSeek".to_owned()),
                model: req.model.unwrap_or_else(|| "deepseek-chat".to_owned()),
                latency_ms: 12,
                message: "ok".to_owned(),
            })
        }

        async fn list_ai_models(
            &self,
            _: Uuid,
            req: AiModelsRequest,
        ) -> Result<AiModelsResult, CoreError> {
            let base = req
                .base_url
                .unwrap_or_else(|| "https://api.deepseek.com".to_owned());
            Ok(AiModelsResult {
                models: vec![
                    digest_core::traits::AiModelOption {
                        id: "deepseek-chat".to_owned(),
                        owned_by: Some("deepseek".to_owned()),
                    },
                    digest_core::traits::AiModelOption {
                        id: "deepseek-reasoner".to_owned(),
                        owned_by: Some("deepseek".to_owned()),
                    },
                ],
                endpoint_url: format!("{}/v1/models", base.trim_end_matches('/')),
                latency_ms: 10,
            })
        }

        async fn test_source(
            &self,
            _: Uuid,
            source_id: Uuid,
        ) -> Result<SourceTestResult, CoreError> {
            Ok(SourceTestResult {
                source_id,
                outcome: FetchOutcome::Updated,
                items_fetched: 1,
                sample_titles: vec!["Sample Item".to_owned()],
                error_message: None,
            })
        }

        async fn test_channel(
            &self,
            _: Uuid,
            channel_id: Uuid,
        ) -> Result<ChannelTestResult, CoreError> {
            Ok(ChannelTestResult {
                channel_id,
                channel_type: NotificationChannelType::Webhook,
                status: DeliveryStatus::Sent,
                error_message: None,
                sent_at: Utc::now(),
            })
        }

        async fn resend_delivery(
            &self,
            user_id: Uuid,
            delivery_id: Uuid,
        ) -> Result<DeliveryLogRecord, CoreError> {
            let existing = self
                .store
                .get_delivery_by_id(user_id, delivery_id)
                .await?
                .ok_or_else(|| CoreError::NotFound("delivery not found".to_owned()))?;
            Ok(DeliveryLogRecord {
                status: DeliveryStatus::Sent,
                error_message: None,
                sent_at: Utc::now(),
                ..existing
            })
        }
    }

    fn build_test_state() -> (AppState, Arc<MockStore>, Arc<MockSse>) {
        let core = AppConfig {
            auth_mode: AuthMode::DevBypass,
            ..AppConfig::default()
        };
        let config = Arc::new(ServerConfig {
            core,
            oidc_issuer_url: Some("https://auth.example.com/application/o/digest".to_owned()),
            oidc_client_id: Some("digest-web".to_owned()),
            ..ServerConfig::default()
        });
        let cipher = Arc::new(EnvelopeCipher::from_bytes([0x22u8; CRYPTO_KEY_BYTE_LEN]));
        let store = Arc::new(MockStore::new(cipher.clone()));
        let repos = Repositories {
            users: store.clone(),
            sources: store.clone(),
            digests: store.clone(),
            runs: store.clone(),
            settings: store.clone(),
        };
        let http = reqwest::Client::new();
        let auth = Arc::new(HybridAuthService::new(
            config.clone(),
            store.clone(),
            http.clone(),
        ));
        let sse = Arc::new(MockSse::new());
        let executor = Arc::new(MockExecutor {
            store: store.clone(),
        });
        let state = AppState {
            config,
            repos,
            cipher,
            ssrf: SsrfGuard::new(vec!["blog.google".to_owned()]),
            cache: Arc::new(InMemoryCacheCoordinator::new()),
            auth,
            sse: sse.clone(),
            executor,
            http,
        };
        (state, store, sse)
    }

    fn loopback_req(
        method: http::Method,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(val) => {
                builder = builder.header(CONTENT_TYPE, "application/json");
                Body::from(serde_json::to_vec(&val).unwrap())
            }
            None => Body::empty(),
        };
        let mut req = builder.body(body).unwrap();
        req.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            54321,
        )));
        req
    }

    async fn read_json(resp: http::Response<Body>) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_handler_returns_ok_and_service_name() {
        let Json(payload) = health_handler().await;
        assert_eq!(payload.status, HEALTH_STATUS_OK);
        assert_eq!(payload.service, SERVICE_NAME);
    }

    #[tokio::test]
    async fn full_section_5_api_routes_work_and_enforce_security_invariants() {
        let (state, store, sse) = build_test_state();
        let app = router(state);

        // 1. Public /api/v1/auth/config
        let cfg_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cfg_resp.status(), StatusCode::OK);
        let cfg_json = read_json(cfg_resp).await;
        assert_eq!(cfg_json["authMode"], "dev_bypass");
        assert_eq!(cfg_json["clientId"], "digest-web");

        // 2. /api/v1/me provisions dev user & returns onboarding flags
        let me_resp = app
            .clone()
            .oneshot(loopback_req(http::Method::GET, "/api/v1/me", None))
            .await
            .unwrap();
        assert_eq!(me_resp.status(), StatusCode::OK);
        let me_json = read_json(me_resp).await;
        assert_eq!(me_json["user"]["oidcSub"], DEV_BYPASS_OIDC_SUB);
        assert_eq!(me_json["onboarding"]["hasAiConfig"], false);
        let user_id: Uuid = me_json["user"]["id"].as_str().unwrap().parse().unwrap();

        // 3. AI Settings: SSRF block, encrypt key, masked hint only
        let ssrf_ai = app
            .clone()
            .oneshot(loopback_req(
                http::Method::PUT,
                "/api/v1/settings/ai",
                Some(serde_json::json!({
                    "providerLabel": "Local",
                    "baseUrl": "http://127.0.0.1:11434",
                    "apiKey": "test-key-placeholder-1234",
                    "model": "llama3"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(ssrf_ai.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let put_ai = app
            .clone()
            .oneshot(loopback_req(
                http::Method::PUT,
                "/api/v1/settings/ai",
                Some(serde_json::json!({
                    "providerLabel": "DeepSeek",
                    "baseUrl": "https://93.184.216.34/v1",
                    "apiKey": "test-key-placeholder-9876",
                    "model": "deepseek-chat"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(put_ai.status(), StatusCode::OK);
        let ai_body = put_ai.into_body().collect().await.unwrap().to_bytes();
        let ai_text = String::from_utf8_lossy(&ai_body);
        assert!(!ai_text.contains("test-key-placeholder-9876"));
        let ai_json: serde_json::Value = serde_json::from_slice(&ai_body).unwrap();
        assert_eq!(ai_json["hasApiKey"], true);
        assert_eq!(ai_json["apiKeyHint"], "test****9876");

        let ai_test_ssrf = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/ai/test",
                Some(serde_json::json!({ "baseUrl": "http://169.254.169.254/v1" })),
            ))
            .await
            .unwrap();
        assert_eq!(ai_test_ssrf.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let ai_models_ssrf = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/ai/models",
                Some(serde_json::json!({ "baseUrl": "http://169.254.169.254/v1" })),
            ))
            .await
            .unwrap();
        assert_eq!(ai_models_ssrf.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let ai_models_ok = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/ai/models",
                Some(serde_json::json!({ "baseUrl": "https://93.184.216.34/v1" })),
            ))
            .await
            .unwrap();
        assert_eq!(ai_models_ok.status(), StatusCode::OK);
        let models_json = read_json(ai_models_ok).await;
        assert_eq!(models_json["models"][0]["id"], "deepseek-chat");

        // 4. Sources CRUD, preset import, export, import, test
        let preset_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/sources/import-preset",
                Some(serde_json::json!({ "canonicalKeys": ["google-ai-blog"] })),
            ))
            .await
            .unwrap();
        assert_eq!(preset_resp.status(), StatusCode::OK);
        let preset_json = read_json(preset_resp).await;
        assert_eq!(preset_json["importedCount"], 1);
        let source_id = preset_json["sources"][0]["id"].as_str().unwrap().to_owned();

        let export_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/sources/export",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(export_resp.status(), StatusCode::OK);
        let export_json = read_json(export_resp).await;
        assert_eq!(export_json["version"], 1);

        let import_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/sources/import",
                Some(export_json),
            ))
            .await
            .unwrap();
        assert_eq!(import_resp.status(), StatusCode::OK);

        let test_src_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                &format!("/api/v1/settings/sources/{source_id}/test"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(test_src_resp.status(), StatusCode::OK);

        // Private source with credentials: SSRF blocked on local URL, masked hint on public IP
        let ssrf_src = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/sources",
                Some(serde_json::json!({
                    "sourceType": "rss",
                    "handleOrUrl": "http://127.0.0.1/private.xml"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(ssrf_src.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let priv_src = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/sources",
                Some(serde_json::json!({
                    "sourceType": "x_account",
                    "customName": "Private X",
                    "handleOrUrl": "GoogleDeepMind",
                    "credentials": { "bearerToken": "test-x-bearer-secret-9999" }
                })),
            ))
            .await
            .unwrap();
        assert_eq!(priv_src.status(), StatusCode::OK);
        let priv_bytes = priv_src.into_body().collect().await.unwrap().to_bytes();
        let priv_text = String::from_utf8_lossy(&priv_bytes);
        assert!(!priv_text.contains("test-x-bearer-secret-9999"));
        let priv_json: serde_json::Value = serde_json::from_slice(&priv_bytes).unwrap();
        assert_eq!(priv_json["scope"], "user_private");
        assert_eq!(priv_json["hasCredentials"], true);
        assert_eq!(priv_json["configHint"], "test****9999");
        let priv_id = priv_json["id"].as_str().unwrap().to_owned();

        let put_src = app
            .clone()
            .oneshot(loopback_req(
                http::Method::PUT,
                &format!("/api/v1/settings/sources/{priv_id}"),
                Some(serde_json::json!({ "enabled": false, "customName": "Paused X" })),
            ))
            .await
            .unwrap();
        assert_eq!(put_src.status(), StatusCode::OK);
        assert_eq!(read_json(put_src).await["enabled"], false);

        let del_src = app
            .clone()
            .oneshot(loopback_req(
                http::Method::DELETE,
                &format!("/api/v1/settings/sources/{priv_id}"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(del_src.status(), StatusCode::NO_CONTENT);

        // 5. Notification Channels: SSRF & SMTP validation + masked configHint
        let bad_chan = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/channels",
                Some(serde_json::json!({
                    "channelType": "webhook",
                    "name": "Local Hook",
                    "config": { "url": "http://127.0.0.1:9999/hook" }
                })),
            ))
            .await
            .unwrap();
        assert_eq!(bad_chan.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let smtp_chan = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/channels",
                Some(serde_json::json!({
                    "channelType": "smtp",
                    "name": "Work Mail",
                    "config": {
                        "host": "93.184.216.34",
                        "port": 587,
                        "username": "mailer@example.com",
                        "password": "test-smtp-secret-password",
                        "from": "digest@example.com",
                        "to": ["alice@example.com"]
                    }
                })),
            ))
            .await
            .unwrap();
        assert_eq!(smtp_chan.status(), StatusCode::OK);
        let smtp_bytes = smtp_chan.into_body().collect().await.unwrap().to_bytes();
        let smtp_text = String::from_utf8_lossy(&smtp_bytes);
        assert!(!smtp_text.contains("test-smtp-secret-password"));
        let smtp_json: serde_json::Value = serde_json::from_slice(&smtp_bytes).unwrap();
        let channel_id = smtp_json["id"].as_str().unwrap().to_owned();

        let chan_test = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                &format!("/api/v1/settings/channels/{channel_id}/test"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(chan_test.status(), StatusCode::OK);

        // 6. Schedule & Watchlist
        let bad_sched = app
            .clone()
            .oneshot(loopback_req(
                http::Method::PUT,
                "/api/v1/settings/schedule",
                Some(serde_json::json!({
                    "timezone": "Invalid/Zone",
                    "dailyTime": "08:30",
                    "emailEnabled": true,
                    "sendWhenEmpty": false,
                    "alertScanEnabled": true,
                    "alertIntervalMinutes": 60
                })),
            ))
            .await
            .unwrap();
        assert_eq!(bad_sched.status(), StatusCode::BAD_REQUEST);

        let ok_sched = app
            .clone()
            .oneshot(loopback_req(
                http::Method::PUT,
                "/api/v1/settings/schedule",
                Some(serde_json::json!({
                    "timezone": "Asia/Shanghai",
                    "dailyTime": "08:30",
                    "emailEnabled": true,
                    "sendWhenEmpty": false,
                    "alertScanEnabled": true,
                    "alertIntervalMinutes": 30
                })),
            ))
            .await
            .unwrap();
        assert_eq!(ok_sched.status(), StatusCode::OK);

        let wl_create = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/watchlist",
                Some(serde_json::json!({
                    "name": "Gemini Releases",
                    "keywords": ["gemini-2.5", "sdk"],
                    "excludeKeywords": ["spam"],
                    "priority": "urgent",
                    "notifyImmediately": true,
                    "enabled": true
                })),
            ))
            .await
            .unwrap();
        assert_eq!(wl_create.status(), StatusCode::OK);

        // 7. Digests, Markdown export, Source Digests/Items, Search, Bookmarks
        let date = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        store.digests.lock().await.push(DailyDigestDetail {
            id: Uuid::new_v4(),
            digest_date: date,
            input_fingerprint: "fp-1".to_owned(),
            title: "Gemini Daily Digest".to_owned(),
            overview_markdown: "Overview summary.".to_owned(),
            overview_html: "<p>Overview summary.</p>".to_owned(),
            sections: Vec::new(),
            item_count: 1,
            highlighted_count: 0,
            prompt_tokens: 10,
            completion_tokens: 5,
            model_used: "deepseek-chat".to_owned(),
            degraded_fallback: false,
            created_at: Utc::now(),
        });

        let md_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::GET,
                "/api/v1/digests/2026-11-26/markdown",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(md_resp.status(), StatusCode::OK);
        assert_eq!(
            md_resp
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some(CONTENT_TYPE_MARKDOWN_UTF8)
        );
        let md_body = md_resp.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&md_body).contains("# Gemini Daily Digest"));

        let missing_digest = app
            .clone()
            .oneshot(loopback_req(
                http::Method::GET,
                "/api/v1/digests/2026-01-01",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(missing_digest.status(), StatusCode::NOT_FOUND);

        let bad_page = app
            .clone()
            .oneshot(loopback_req(
                http::Method::GET,
                "/api/v1/digests?page=0",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(bad_page.status(), StatusCode::BAD_REQUEST);
        let bad_page_json = read_json(bad_page).await;
        assert_eq!(bad_page_json["code"], ErrorCode::InvalidInput.as_str());

        let src_digests_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::GET,
                &format!("/api/v1/sources/{source_id}/digests"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(src_digests_resp.status(), StatusCode::OK);

        let src_items_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::GET,
                &format!("/api/v1/sources/{source_id}/items"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(src_items_resp.status(), StatusCode::OK);

        let search_items_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::GET,
                "/api/v1/items?q=gemini&dateFrom=2026-11-01&dateTo=2026-11-26&bookmarked=false",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(search_items_resp.status(), StatusCode::OK);

        let item_id = Uuid::new_v4();
        let bm_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/bookmarks",
                Some(serde_json::json!({ "itemId": item_id, "note": "Important" })),
            ))
            .await
            .unwrap();
        assert_eq!(bm_resp.status(), StatusCode::OK);

        let bm_del = app
            .clone()
            .oneshot(loopback_req(
                http::Method::DELETE,
                &format!("/api/v1/bookmarks/{item_id}"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(bm_del.status(), StatusCode::NO_CONTENT);

        // 8. Runs & Delivery resend
        let trigger_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/runs/trigger",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(trigger_resp.status(), StatusCode::OK);

        let stats_resp = app
            .clone()
            .oneshot(loopback_req(http::Method::GET, "/api/v1/runs/stats", None))
            .await
            .unwrap();
        assert_eq!(stats_resp.status(), StatusCode::OK);

        let delivery_id = Uuid::new_v4();
        store.deliveries.lock().await.push(DeliveryLogRecord {
            id: delivery_id,
            run_id: Uuid::new_v4(),
            user_id,
            digest_id: None,
            channel_id: None,
            channel_type: NotificationChannelType::Webhook,
            status: DeliveryStatus::Failed,
            error_message: Some("timeout".to_owned()),
            sent_at: Utc::now(),
        });
        let resend_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                &format!("/api/v1/deliveries/{delivery_id}/resend"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resend_resp.status(), StatusCode::OK);
        assert_eq!(read_json(resend_resp).await["status"], "sent");

        // 9. SSE stream receives published events
        let sse_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::GET,
                "/api/v1/events/stream",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(sse_resp.status(), StatusCode::OK);
        sse.publish(
            user_id,
            SseEnvelope {
                event_type: SseEventType::RunProgress,
                payload: serde_json::json!({ "stage": "fetching" }),
            },
        );
        let mut body = sse_resp.into_body();
        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let data = frame.into_data().unwrap();
        let chunk = String::from_utf8_lossy(&data);
        assert!(chunk.contains("event: run_progress"));
        assert!(chunk.contains("fetching"));
    }

    #[tokio::test]
    async fn static_dir_serves_spa_assets_and_isolates_api_404_and_traversal() {
        let root = tempfile::tempdir().unwrap();
        let static_dir = root.path().join("dist");
        let assets_dir = static_dir.join("assets");
        std::fs::create_dir_all(&assets_dir).unwrap();
        std::fs::write(
            static_dir.join(SPA_INDEX_FILE),
            "<!doctype html><html><body>SPA_INDEX</body></html>",
        )
        .unwrap();
        std::fs::write(assets_dir.join("app.js"), "console.log('spa-asset');").unwrap();
        std::fs::write(root.path().join("outside.txt"), "OUTSIDE_SECRET").unwrap();

        let (mut state, _store, _sse) = build_test_state();
        let mut cfg = (*state.config).clone();
        cfg.core.static_dir = Some(static_dir);
        state.config = Arc::new(cfg);

        let app = router(state);

        // 1. Root serves index.html (200)
        let root_resp = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(root_resp.status(), StatusCode::OK);
        let root_body =
            String::from_utf8_lossy(&root_resp.into_body().collect().await.unwrap().to_bytes())
                .to_string();
        assert!(root_body.contains("SPA_INDEX"));

        // 2. Static asset serves JS file (200)
        let asset_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/assets/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(asset_resp.status(), StatusCode::OK);
        let asset_body =
            String::from_utf8_lossy(&asset_resp.into_body().collect().await.unwrap().to_bytes())
                .to_string();
        assert!(asset_body.contains("spa-asset"));

        // 3. SPA client-side route falls back to index.html (200)
        let spa_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/callback?code=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(spa_resp.status(), StatusCode::OK);
        let spa_body =
            String::from_utf8_lossy(&spa_resp.into_body().collect().await.unwrap().to_bytes())
                .to_string();
        assert!(spa_body.contains("SPA_INDEX"));

        // 4. Unmatched /api and /api/v1/* return 404 JSON ErrorEnvelope (never SPA index.html)
        for missing_api in [
            "/api",
            "/api/",
            "/api/missing",
            "/api/v1",
            "/api/v1/",
            "/api/v1/missing",
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(missing_api)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "expected 404 for {missing_api}"
            );
            let json = read_json(resp).await;
            assert_eq!(
                json["code"],
                ErrorCode::NotFound.as_str(),
                "expected NOT_FOUND code for {missing_api}"
            );
            assert!(json["requestId"].as_str().is_some_and(|s| !s.is_empty()));
        }

        // 5. Path traversal never reads files outside STATIC_DIR
        for traversal in ["/../outside.txt", "/%2e%2e/outside.txt"] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(traversal)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let body_text = String::from_utf8_lossy(&body_bytes);
            assert!(
                !body_text.contains("OUTSIDE_SECRET"),
                "traversal leaked outside file for {traversal}"
            );
        }
    }

    #[tokio::test]
    async fn admin_platform_endpoints_require_admin_and_hot_reload_bridge() {
        use wiremock::matchers::{body_json, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let reload_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/internal/twitter-token"))
            .and(header("authorization", "Bearer reload-secret-123"))
            .and(body_json(serde_json::json!({
                "authToken": "fresh_x_token_value"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true
            })))
            .expect(1)
            .mount(&reload_server)
            .await;

        let (mut state, store, _sse) = build_test_state();
        let mut cfg = (*state.config).clone();
        cfg.core.ssrf_allowlist_hosts = vec!["127.0.0.1".to_owned(), "localhost".to_owned()];
        state.config = Arc::new(cfg);
        state.ssrf = SsrfGuard::from_config(&state.config.core);
        let app = router(state);

        // First authenticate as dev_bypass (which is admin), verify GET succeeds
        let get_admin = app
            .clone()
            .oneshot(loopback_req(
                http::Method::GET,
                "/api/v1/admin/platform",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(get_admin.status(), StatusCode::OK);
        let initial_view = read_json(get_admin).await;
        assert_eq!(initial_view["hasXAuthToken"], false);

        // Verify X presets are paused when platform X is unconfigured
        let preset_paused_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/sources/import-preset",
                Some(serde_json::json!({ "includeXTemplates": true })),
            ))
            .await
            .unwrap();
        assert_eq!(preset_paused_resp.status(), StatusCode::OK);
        let preset_paused_json = read_json(preset_paused_resp).await;
        let x_paused: Vec<_> = preset_paused_json["sources"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["sourceType"] == "x_account" || s["sourceType"] == "x_search")
            .collect();
        assert_eq!(x_paused.len(), 3);
        assert!(x_paused.iter().all(|s| s["enabled"] == false));

        // Update platform X settings using a full pasted Cookie header -> extracts auth_token and hot-reloads bridge
        let put_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::PUT,
                "/api/v1/admin/platform",
                Some(serde_json::json!({
                    "xBridgeBaseUrl": reload_server.uri(),
                    "xBridgeReloadUrl": format!("{}/internal/twitter-token", reload_server.uri()),
                    "xBridgeReloadSecret": "reload-secret-123",
                    "xAuthToken": "guest_id=v1%3A1; auth_token=fresh_x_token_value; ct0=csrf_token;"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(put_resp.status(), StatusCode::OK);
        let put_json = read_json(put_resp).await;
        assert_eq!(put_json["bridgeReloaded"], true);
        assert_eq!(put_json["settings"]["hasXAuthToken"], true);
        assert_eq!(put_json["settings"]["xBridgeConfigured"], true);
        assert_eq!(put_json["settings"]["xIntegrationReady"], true);
        assert!(!put_json.to_string().contains("fresh_x_token_value"));
        assert!(!put_json.to_string().contains("reload-secret-123"));

        // Clear earlier paused subscriptions and re-import presets: X templates are now enabled automatically
        store.sources.lock().await.clear();
        let preset_ready_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/settings/sources/import-preset",
                Some(serde_json::json!({ "includeXTemplates": true })),
            ))
            .await
            .unwrap();
        assert_eq!(preset_ready_resp.status(), StatusCode::OK);
        let preset_ready_json = read_json(preset_ready_resp).await;
        assert!(preset_ready_json["sources"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["sourceType"] == "x_account" || s["sourceType"] == "x_search")
            .all(|s| s["enabled"] == true));

        // Live platform X test works
        let test_resp = app
            .clone()
            .oneshot(loopback_req(
                http::Method::POST,
                "/api/v1/admin/platform/x/test",
                Some(serde_json::json!({ "handle": "@GoogleDeepMind" })),
            ))
            .await
            .unwrap();
        assert_eq!(test_resp.status(), StatusCode::OK);
        let test_json = read_json(test_resp).await;
        assert_eq!(test_json["mode"], "rsshub_bridge");
        assert_eq!(test_json["handle"], "GoogleDeepMind");

        // Non-admin user receives 403 Forbidden
        {
            let mut users = store.users.lock().await;
            for u in users.values_mut() {
                u.role = "member".to_owned();
            }
        }
        let forbidden_resp = app
            .oneshot(loopback_req(
                http::Method::GET,
                "/api/v1/admin/platform",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(forbidden_resp.status(), StatusCode::FORBIDDEN);
    }
}
