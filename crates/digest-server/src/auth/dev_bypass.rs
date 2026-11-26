//! Loopback-only local development authentication (`AuthMode::DevBypass`).
//!
//! Strictly inspects the TCP peer `SocketAddr` and NEVER trusts forwarded headers.

use std::net::SocketAddr;

use digest_core::error::CoreError;

use crate::constants::{
    ADMIN_USER_ROLE, DEV_BYPASS_DISPLAY_NAME, DEV_BYPASS_EMAIL, DEV_BYPASS_OIDC_SUB,
};
use crate::repo::{UpsertUserCmd, UserRecord, UserRepo};

/// Validates that `peer_addr` is present and a loopback IP (`127.0.0.1` / `::1`),
/// then upserts and returns the deterministic local development user.
pub async fn authenticate_dev_bypass(
    users: &dyn UserRepo,
    peer_addr: Option<SocketAddr>,
) -> Result<UserRecord, CoreError> {
    match peer_addr {
        Some(addr) if addr.ip().is_loopback() => {}
        Some(_) => {
            return Err(CoreError::Forbidden(
                "dev_bypass authentication is restricted to loopback connections".to_owned(),
            ));
        }
        None => {
            return Err(CoreError::Unauthorized(
                "missing peer socket address for dev_bypass authentication".to_owned(),
            ));
        }
    }

    users
        .upsert_oidc_user(UpsertUserCmd {
            oidc_sub: DEV_BYPASS_OIDC_SUB.to_owned(),
            email: Some(DEV_BYPASS_EMAIL.to_owned()),
            display_name: Some(DEV_BYPASS_DISPLAY_NAME.to_owned()),
            role: ADMIN_USER_ROLE.to_owned(),
        })
        .await
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::routing::get;
    use axum::Router;
    use chrono::Utc;
    use digest_core::cache::InMemoryCacheCoordinator;
    use digest_core::config::AppConfig;
    use digest_core::constants::CRYPTO_KEY_BYTE_LEN;
    use digest_core::crypto::EnvelopeCipher;
    use digest_core::domain::{
        AuthMode, DeliveryStatus, NotificationChannelType, RunStatus, RunType, SourceScope,
    };
    use digest_core::ssrf::SsrfGuard;
    use http::{Request, StatusCode};
    use tokio::sync::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::*;
    use crate::auth::{AuthenticatedUser, HybridAuthService};
    use crate::config::ServerConfig;
    use crate::repo::*;
    use crate::services::{
        AiTestRequest, AiTestResult, ChannelTestResult, ExecutionOrchestrator, SourceTestResult,
        SseEnvelope, SsePublisher, TriggerRunRequest,
    };
    use crate::state::AppState;

    #[derive(Default)]
    struct StubUserRepo {
        calls: Mutex<Vec<UpsertUserCmd>>,
    }

    #[async_trait]
    impl UserRepo for StubUserRepo {
        async fn upsert_oidc_user(&self, cmd: UpsertUserCmd) -> Result<UserRecord, CoreError> {
            let mut guard = self.calls.lock().await;
            guard.push(cmd.clone());
            Ok(UserRecord {
                id: Uuid::nil(),
                oidc_sub: cmd.oidc_sub,
                email: cmd.email,
                display_name: cmd.display_name,
                role: cmd.role,
                preferred_locale: digest_core::domain::default_digest_locale(),
                created_at: Utc::now(),
                last_login_at: Some(Utc::now()),
            })
        }

        async fn get_user_by_id(&self, _user_id: Uuid) -> Result<Option<UserRecord>, CoreError> {
            Ok(None)
        }

        async fn update_user_locale(
            &self,
            _user_id: Uuid,
            _preferred_locale: &str,
        ) -> Result<UserRecord, CoreError> {
            Err(CoreError::NotFound("stub".to_owned()))
        }

        async fn get_onboarding_flags(&self, _user_id: Uuid) -> Result<OnboardingFlags, CoreError> {
            Ok(OnboardingFlags::default())
        }
    }

    struct DummyRepos;

    #[async_trait]
    impl SourceRepo for DummyRepos {
        async fn create_or_subscribe(
            &self,
            _: Uuid,
            _: SubscribeSourceCmd,
        ) -> Result<UserSourceView, CoreError> {
            unreachable!()
        }
        async fn list_user_sources(&self, _: Uuid) -> Result<Vec<UserSourceView>, CoreError> {
            Ok(Vec::new())
        }
        async fn get_user_source(
            &self,
            _: Uuid,
            _: Uuid,
        ) -> Result<Option<UserSourceRecord>, CoreError> {
            Ok(None)
        }
        async fn update_user_subscription(
            &self,
            _: Uuid,
            _: Uuid,
            _: UpdateSourceCmd,
        ) -> Result<UserSourceView, CoreError> {
            unreachable!()
        }
        async fn delete_user_subscription(&self, _: Uuid, _: Uuid) -> Result<(), CoreError> {
            Ok(())
        }
        async fn list_pollable_sources(
            &self,
            _: Option<SourceScope>,
            _: chrono::DateTime<Utc>,
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
            _: chrono::DateTime<Utc>,
            _: usize,
        ) -> Result<Vec<CanonicalItemRecord>, CoreError> {
            Ok(Vec::new())
        }
        async fn list_source_items(
            &self,
            _: Uuid,
            _: Uuid,
            page: PageRequest,
        ) -> Result<PagedResult<ItemView>, CoreError> {
            Ok(PagedResult::new(Vec::new(), page, 0))
        }
        async fn search_user_items(
            &self,
            _: Uuid,
            q: ItemSearchQuery,
        ) -> Result<PagedResult<ItemView>, CoreError> {
            Ok(PagedResult::new(Vec::new(), q.page, 0))
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
            _: chrono::DateTime<Utc>,
        ) -> Result<(), CoreError> {
            Ok(())
        }
        async fn prune_unbookmarked_bodies(
            &self,
            _: chrono::DateTime<Utc>,
        ) -> Result<u64, CoreError> {
            Ok(0)
        }
        async fn upsert_bookmark(
            &self,
            _: Uuid,
            _: Uuid,
            _: Option<String>,
        ) -> Result<BookmarkView, CoreError> {
            unreachable!()
        }
        async fn delete_bookmark(&self, _: Uuid, _: Uuid) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[async_trait]
    impl DigestRepo for DummyRepos {
        async fn list_daily_digests(
            &self,
            _: Uuid,
            page: PageRequest,
        ) -> Result<PagedResult<DailyDigestSummary>, CoreError> {
            Ok(PagedResult::new(Vec::new(), page, 0))
        }
        async fn get_daily_digest_by_date(
            &self,
            _: Uuid,
            _: chrono::NaiveDate,
        ) -> Result<Option<DailyDigestDetail>, CoreError> {
            Ok(None)
        }
        async fn find_daily_digest_by_fingerprint(
            &self,
            _: Uuid,
            _: chrono::NaiveDate,
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
            _: Uuid,
            page: PageRequest,
        ) -> Result<PagedResult<SourceDailyDigestView>, CoreError> {
            Ok(PagedResult::new(Vec::new(), page, 0))
        }
    }

    #[async_trait]
    impl RunRepo for DummyRepos {
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
            Ok(PagedResult::new(Vec::new(), page, 0))
        }
        async fn record_delivery(
            &self,
            _: CreateDeliveryLogCmd,
        ) -> Result<DeliveryLogRecord, CoreError> {
            unreachable!()
        }
        async fn get_delivery_by_id(
            &self,
            _: Uuid,
            _: Uuid,
        ) -> Result<Option<DeliveryLogRecord>, CoreError> {
            Ok(None)
        }
        async fn update_delivery_status(
            &self,
            _: Uuid,
            _: Uuid,
            _: DeliveryStatus,
            _: Option<String>,
            _: chrono::DateTime<Utc>,
        ) -> Result<DeliveryLogRecord, CoreError> {
            unreachable!()
        }
        async fn get_run_stats(&self, _: Uuid) -> Result<RunStatsView, CoreError> {
            unreachable!()
        }
    }

    #[async_trait]
    impl SettingsRepo for DummyRepos {
        async fn get_ai_config(&self, _: Uuid) -> Result<Option<UserAiConfigRecord>, CoreError> {
            Ok(None)
        }
        async fn upsert_ai_config(
            &self,
            _: Uuid,
            _: UpsertAiConfigRecord,
        ) -> Result<UserAiConfigRecord, CoreError> {
            unreachable!()
        }
        async fn list_channels(
            &self,
            _: Uuid,
        ) -> Result<Vec<NotificationChannelRecord>, CoreError> {
            Ok(Vec::new())
        }
        async fn get_channel(
            &self,
            _: Uuid,
            _: Uuid,
        ) -> Result<Option<NotificationChannelRecord>, CoreError> {
            Ok(None)
        }
        async fn create_channel(
            &self,
            _: Uuid,
            _: CreateChannelRecord,
        ) -> Result<NotificationChannelRecord, CoreError> {
            unreachable!()
        }
        async fn update_channel(
            &self,
            _: Uuid,
            _: Uuid,
            _: UpdateChannelRecord,
        ) -> Result<NotificationChannelRecord, CoreError> {
            unreachable!()
        }
        async fn delete_channel(&self, _: Uuid, _: Uuid) -> Result<(), CoreError> {
            Ok(())
        }
        async fn get_schedule(&self, _: Uuid) -> Result<UserScheduleRecord, CoreError> {
            unreachable!()
        }
        async fn upsert_schedule(
            &self,
            _: Uuid,
            _: UpdateScheduleCmd,
        ) -> Result<UserScheduleRecord, CoreError> {
            unreachable!()
        }
        async fn list_due_daily_schedules(&self) -> Result<Vec<UserScheduleRecord>, CoreError> {
            Ok(Vec::new())
        }
        async fn mark_daily_run_date(
            &self,
            _: Uuid,
            _: chrono::NaiveDate,
        ) -> Result<(), CoreError> {
            Ok(())
        }
        async fn list_due_alert_schedules(
            &self,
            _: chrono::DateTime<Utc>,
        ) -> Result<Vec<UserScheduleRecord>, CoreError> {
            Ok(Vec::new())
        }
        async fn mark_alert_scan_at(
            &self,
            _: Uuid,
            _: chrono::DateTime<Utc>,
        ) -> Result<(), CoreError> {
            Ok(())
        }
        async fn list_watchlist_rules(
            &self,
            _: Uuid,
        ) -> Result<Vec<WatchlistRuleRecord>, CoreError> {
            Ok(Vec::new())
        }
        async fn create_watchlist_rule(
            &self,
            _: Uuid,
            _: UpsertWatchlistRuleCmd,
        ) -> Result<WatchlistRuleRecord, CoreError> {
            unreachable!()
        }
        async fn update_watchlist_rule(
            &self,
            _: Uuid,
            _: Uuid,
            _: UpsertWatchlistRuleCmd,
        ) -> Result<WatchlistRuleRecord, CoreError> {
            unreachable!()
        }
        async fn delete_watchlist_rule(&self, _: Uuid, _: Uuid) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct DummySse;
    impl SsePublisher for DummySse {
        fn publish(&self, _: Uuid, _: SseEnvelope) {}
        fn subscribe(&self, _: Uuid) -> tokio::sync::broadcast::Receiver<SseEnvelope> {
            tokio::sync::broadcast::channel(1).1
        }
    }

    struct DummyExec;
    #[async_trait]
    impl ExecutionOrchestrator for DummyExec {
        async fn trigger_manual_run(
            &self,
            _: Uuid,
            _: TriggerRunRequest,
        ) -> Result<DigestRunView, CoreError> {
            Ok(DigestRunView {
                id: Uuid::new_v4(),
                run_type: RunType::ManualDaily,
                status: RunStatus::Succeeded,
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                items_fetched: 0,
                items_new: 0,
                prompt_tokens: 0,
                completion_tokens: 0,
                error_summary: None,
                deliveries: Vec::new(),
            })
        }
        async fn test_ai_config(
            &self,
            _: Uuid,
            _: AiTestRequest,
        ) -> Result<AiTestResult, CoreError> {
            unreachable!()
        }
        async fn test_source(&self, _: Uuid, _: Uuid) -> Result<SourceTestResult, CoreError> {
            unreachable!()
        }
        async fn test_channel(&self, _: Uuid, _: Uuid) -> Result<ChannelTestResult, CoreError> {
            Ok(ChannelTestResult {
                channel_id: Uuid::new_v4(),
                channel_type: NotificationChannelType::Webhook,
                status: DeliveryStatus::Sent,
                error_message: None,
                sent_at: Utc::now(),
            })
        }
        async fn resend_delivery(&self, _: Uuid, _: Uuid) -> Result<DeliveryLogRecord, CoreError> {
            unreachable!()
        }
    }

    fn build_dev_bypass_state() -> AppState {
        let core = AppConfig {
            auth_mode: AuthMode::DevBypass,
            ..AppConfig::default()
        };
        let config = Arc::new(ServerConfig {
            core,
            ..ServerConfig::default()
        });
        let users = Arc::new(StubUserRepo::default());
        let dummy = Arc::new(DummyRepos);
        let repos = Repositories {
            users: users.clone(),
            sources: dummy.clone(),
            digests: dummy.clone(),
            runs: dummy.clone(),
            settings: dummy,
        };
        let http = reqwest::Client::new();
        let auth = Arc::new(HybridAuthService::new(config.clone(), users, http.clone()));
        AppState {
            config,
            repos,
            cipher: Arc::new(EnvelopeCipher::from_bytes([0x11u8; CRYPTO_KEY_BYTE_LEN])),
            ssrf: SsrfGuard::new(Vec::new()),
            cache: Arc::new(InMemoryCacheCoordinator::new()),
            auth,
            sse: Arc::new(DummySse),
            executor: Arc::new(DummyExec),
            http,
        }
    }

    #[tokio::test]
    async fn loopback_ipv4_and_ipv6_succeed_and_missing_or_remote_peer_fail_closed() {
        let repo = StubUserRepo::default();

        let v4_ok = authenticate_dev_bypass(
            &repo,
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 51234)),
        )
        .await
        .unwrap();
        assert_eq!(v4_ok.oidc_sub, DEV_BYPASS_OIDC_SUB);
        assert_eq!(v4_ok.email.as_deref(), Some(DEV_BYPASS_EMAIL));

        let v6_ok = authenticate_dev_bypass(
            &repo,
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 51235)),
        )
        .await
        .unwrap();
        assert_eq!(v6_ok.oidc_sub, DEV_BYPASS_OIDC_SUB);

        let remote_err = authenticate_dev_bypass(
            &repo,
            Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
                8080,
            )),
        )
        .await
        .unwrap_err();
        assert!(matches!(remote_err, CoreError::Forbidden(_)));

        let missing_err = authenticate_dev_bypass(&repo, None).await.unwrap_err();
        assert!(matches!(missing_err, CoreError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn extractor_ignores_spoofed_forwarded_headers_and_fails_closed_without_connect_info() {
        async fn protected(AuthenticatedUser(user): AuthenticatedUser) -> String {
            user.oidc_sub
        }

        let state = build_dev_bypass_state();
        let app = Router::new()
            .route("/protected", get(protected))
            .with_state(state);

        // 127.0.0.1 succeeds
        let mut ok_req = Request::builder()
            .uri("/protected")
            .body(Body::empty())
            .unwrap();
        ok_req.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            40001,
        )));
        let ok_resp = app.clone().oneshot(ok_req).await.unwrap();
        assert_eq!(ok_resp.status(), StatusCode::OK);

        // 10.0.0.5 with spoofed X-Forwarded-For / X-Real-IP / Forwarded / Host MUST fail closed with 403
        let mut spoofed_req = Request::builder()
            .uri("/protected")
            .header("x-forwarded-for", "127.0.0.1")
            .header("forwarded", "for=127.0.0.1")
            .header("x-real-ip", "127.0.0.1")
            .header("host", "127.0.0.1:8080")
            .body(Body::empty())
            .unwrap();
        spoofed_req
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
                40002,
            )));
        let spoofed_resp = app.clone().oneshot(spoofed_req).await.unwrap();
        assert_eq!(spoofed_resp.status(), StatusCode::FORBIDDEN);

        // Missing ConnectInfo MUST fail closed with 401 (never panic!)
        let missing_req = Request::builder()
            .uri("/protected")
            .body(Body::empty())
            .unwrap();
        let missing_resp = app.oneshot(missing_req).await.unwrap();
        assert_eq!(missing_resp.status(), StatusCode::UNAUTHORIZED);
    }
}
