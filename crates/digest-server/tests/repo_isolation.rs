#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::Router;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chrono::{Duration as ChronoDuration, NaiveDate, Utc};
use digest_core::cache::{CacheCoordinator, InMemoryCacheCoordinator};
use digest_core::collectors::{CollectorContext, CollectorRegistry};
use digest_core::config::{AppConfig, CollectorTuning, LlmTuning, NotifierTuning};
use digest_core::constants::{
    CRYPTO_KEY_BYTE_LEN, HEADER_AUTHORIZATION, HEADER_BEARER_PREFIX, HEADER_CONTENT_TYPE,
};
use digest_core::domain::{AuthMode, ErrorCode, RunType};
use digest_core::error::CoreError;
use digest_core::http::shared_http_client;
use digest_core::llm::{IdempotentLlmRunner, OpenAiCompatProvider};
use digest_core::notifiers::NotifierRegistry;
use digest_core::pipeline::ItemProcessingPipeline;
use digest_core::renderer::RendererRegistry;
use digest_core::ssrf::SsrfGuard;
use digest_core::traits::StructuredLlmProvider;
use digest_server::auth::HybridAuthService;
use digest_server::build_router;
use digest_server::config::ServerConfig;
use digest_server::repo::in_memory::InMemoryRepoStore;
use digest_server::repo::{NewCanonicalItem, Repositories, UpdateSourceCmd};
use digest_server::scheduler::poller::poll_canonical_source_once;
use digest_server::scheduler::retention::run_retention_once;
use digest_server::scheduler::{ComposerDeps, ServerExecutionOrchestrator};
use digest_server::services::{AuthService, ExecutionOrchestrator, SsePublisher};
use digest_server::sse_hub::SseHub;
use digest_server::state::AppState;
use http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use serde::Serialize;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MIGRATION_0001_SQL: &str = include_str!("../migrations/0001_init.sql");

#[derive(Serialize)]
struct OidcClaims {
    sub: String,
    iss: String,
    aud: String,
    exp: usize,
    nbf: usize,
    email: Option<String>,
    name: Option<String>,
}

fn build_rsa_keypair_and_jwk(kid: &str) -> (EncodingKey, serde_json::Value) {
    let mut rng = rand::thread_rng();
    let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let public_key = private_key.to_public_key();
    let pem = private_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
    let n = URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
    let e = URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());
    let jwk = serde_json::json!({
        "kty": "RSA",
        "use": "sig",
        "alg": "RS256",
        "kid": kid,
        "n": n,
        "e": e,
    });
    (encoding_key, jwk)
}

fn mint_token(key: &EncodingKey, issuer: &str, sub: &str, email: &str) -> String {
    let now = usize::try_from(Utc::now().timestamp()).unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("iso-key-1".to_owned());
    jsonwebtoken::encode(
        &header,
        &OidcClaims {
            sub: sub.to_owned(),
            iss: issuer.to_owned(),
            aud: "digest-iso-client".to_owned(),
            exp: now + 600,
            nbf: now.saturating_sub(10),
            email: Some(email.to_owned()),
            name: Some(sub.to_owned()),
        },
        key,
    )
    .unwrap()
}

struct TestHarness {
    app: Router,
    repos: Repositories,
    deps: ComposerDeps,
    orchestrator: ServerExecutionOrchestrator,
}

fn build_oidc_harness(issuer: String) -> TestHarness {
    let mut core = AppConfig {
        auth_mode: AuthMode::Oidc,
        collector: CollectorTuning {
            max_retry_attempts: std::num::NonZeroUsize::new(1).unwrap(),
            initial_backoff: Duration::from_millis(5),
            ..CollectorTuning::default()
        },
        llm: LlmTuning {
            request_timeout: Duration::from_secs(5),
            ..LlmTuning::default()
        },
        ..AppConfig::default()
    };
    core.ssrf_allowlist_hosts = vec!["127.0.0.1".to_owned(), "localhost".to_owned()];

    let config = Arc::new(ServerConfig {
        core,
        database_url: None,
        oidc_issuer_url: Some(issuer),
        oidc_client_id: Some("digest-iso-client".to_owned()),
        oidc_audience: None,
        app_master_key_b64: Some(STANDARD.encode([0x55u8; CRYPTO_KEY_BYTE_LEN])),
    });

    let cipher = Arc::new(config.build_cipher().unwrap());
    let ssrf = SsrfGuard::from_config(&config.core);
    let http = shared_http_client(None, &config.core.collector).unwrap();
    let cache: Arc<dyn CacheCoordinator> = Arc::new(InMemoryCacheCoordinator::new());
    let repos = InMemoryRepoStore::repositories_with_cipher(Arc::clone(&cipher));

    let auth: Arc<dyn AuthService> = Arc::new(HybridAuthService::new(
        Arc::clone(&config),
        Arc::clone(&repos.users),
        http.clone(),
    ));
    let sse: Arc<dyn SsePublisher> = Arc::new(SseHub::new());
    let collectors = Arc::new(CollectorRegistry::new(CollectorContext::new(
        http.clone(),
        ssrf.clone(),
        config.core.collector.clone(),
        config.core.resources.clone(),
    )));
    let pipeline = Arc::new(ItemProcessingPipeline::default());
    let llm_provider: Arc<dyn StructuredLlmProvider> = Arc::new(OpenAiCompatProvider::with_tuning(
        http.clone(),
        ssrf.clone(),
        config.core.llm.clone(),
        config.core.resources.max_feed_bytes.get(),
    ));
    let llm_runner = Arc::new(IdempotentLlmRunner::new(
        Arc::clone(&cache),
        Arc::clone(&llm_provider),
    ));
    let renderer = Arc::new(RendererRegistry::new());
    let notifiers = Arc::new(NotifierRegistry::with_defaults(
        http.clone(),
        ssrf.clone(),
        NotifierTuning::default(),
    ));

    let deps = ComposerDeps {
        config: Arc::clone(&config),
        repos: repos.clone(),
        cipher: Arc::clone(&cipher),
        cache: Arc::clone(&cache),
        collectors,
        pipeline,
        llm_runner,
        renderer,
        notifiers,
        sse: Arc::clone(&sse),
    };
    let orchestrator = ServerExecutionOrchestrator::new(deps.clone(), ssrf.clone(), llm_provider);
    let executor: Arc<dyn ExecutionOrchestrator> = Arc::new(orchestrator.clone());

    let state = AppState {
        config,
        repos: repos.clone(),
        cipher,
        ssrf,
        cache,
        auth,
        sse,
        executor,
        http,
    };

    TestHarness {
        app: build_router(state),
        repos,
        deps,
        orchestrator,
    }
}

fn authed_req(
    method: Method,
    uri: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri).header(
        HEADER_AUTHORIZATION,
        format!("{HEADER_BEARER_PREFIX}{token}"),
    );
    let body = match body {
        Some(value) => {
            builder = builder.header(HEADER_CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn read_json(resp: http::Response<Body>) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn migration_0001_init_sql_defines_14_tables_constraints_and_fts_gin_index() {
    let expected_tables = [
        "CREATE TABLE IF NOT EXISTS users",
        "CREATE TABLE IF NOT EXISTS user_ai_configs",
        "CREATE TABLE IF NOT EXISTS canonical_sources",
        "CREATE TABLE IF NOT EXISTS user_subscriptions",
        "CREATE TABLE IF NOT EXISTS canonical_items",
        "CREATE TABLE IF NOT EXISTS user_item_matches",
        "CREATE TABLE IF NOT EXISTS daily_digests",
        "CREATE TABLE IF NOT EXISTS source_daily_digests",
        "CREATE TABLE IF NOT EXISTS user_notification_channels",
        "CREATE TABLE IF NOT EXISTS user_schedules",
        "CREATE TABLE IF NOT EXISTS user_watchlist_rules",
        "CREATE TABLE IF NOT EXISTS digest_runs",
        "CREATE TABLE IF NOT EXISTS delivery_logs",
        "CREATE TABLE IF NOT EXISTS user_bookmarks",
    ];
    for table_ddl in expected_tables {
        assert!(
            MIGRATION_0001_SQL.contains(table_ddl),
            "missing table DDL: {table_ddl}"
        );
    }
    assert_eq!(
        MIGRATION_0001_SQL
            .matches("CREATE TABLE IF NOT EXISTS ")
            .count(),
        14
    );
    let normalized_sql = MIGRATION_0001_SQL
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(normalized_sql
        .contains("UNIQUE NULLS NOT DISTINCT (source_type, canonical_key, scope_owner_id)"));
    assert!(normalized_sql.contains(
        "CHECK ( (scope = 'shared_public' AND scope_owner_id IS NULL) OR (scope = 'user_private' AND scope_owner_id IS NOT NULL) )"
    ));
    assert!(normalized_sql.contains("search_tsv TSVECTOR"));
    assert!(normalized_sql.contains("canonical_items_update_search_tsv"));
    assert!(normalized_sql.contains("USING GIN (search_tsv)"));
}

#[tokio::test]
async fn multi_tenant_isolation_across_rest_api_and_repo_layer() {
    let server = MockServer::start().await;
    let (encoding_key, jwk) = build_rsa_keypair_and_jwk("iso-key-1");
    let issuer = format!("{}/issuer", server.uri());

    Mock::given(method("GET"))
        .and(path("/issuer/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer": issuer,
            "jwks_uri": format!("{}/jwks.json", server.uri()),
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "keys": [jwk]
        })))
        .mount(&server)
        .await;

    // Shared source fetched ONCE across User A & User B.
    Mock::given(method("GET"))
        .and(path("/shared.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Shared</title><item><guid>s1</guid><title>SharedItem Announcement</title><link>https://example.com/shared/1</link><description>Public shared announcement</description></item></channel></rss>"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    // Private source owned strictly by User A.
    Mock::given(method("GET"))
        .and(path("/private-a.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Private A</title><item><guid>p1</guid><title>PrivateOnlyToken Brief</title><link>https://example.com/priv/1</link><description>Confidential user A note</description></item></channel></rss>"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/hook-a"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let harness = build_oidc_harness(issuer.clone());
    let token_a = mint_token(&encoding_key, &issuer, "tenant-user-a", "usera@example.com");
    let token_b = mint_token(&encoding_key, &issuer, "tenant-user-b", "userb@example.com");

    let me_a = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(Method::GET, "/api/v1/me", &token_a, None))
            .await
            .unwrap(),
    )
    .await;
    let user_a_id: Uuid = me_a["user"]["id"].as_str().unwrap().parse().unwrap();

    let me_b = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(Method::GET, "/api/v1/me", &token_b, None))
            .await
            .unwrap(),
    )
    .await;
    let user_b_id: Uuid = me_b["user"]["id"].as_str().unwrap().parse().unwrap();
    assert_ne!(user_a_id, user_b_id);

    // Both User A and User B subscribe to the same shared_public source.
    let shared_url = format!("{}/shared.xml", server.uri());
    let sub_a = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::POST,
                "/api/v1/settings/sources",
                &token_a,
                Some(serde_json::json!({
                    "sourceType": "rss",
                    "scope": "shared_public",
                    "canonicalKey": "shared-rss-feed",
                    "customName": "User A Shared Name",
                    "category": "Official",
                    "handleOrUrl": shared_url
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let sub_b = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::POST,
                "/api/v1/settings/sources",
                &token_b,
                Some(serde_json::json!({
                    "sourceType": "rss",
                    "scope": "shared_public",
                    "canonicalKey": "shared-rss-feed",
                    "customName": "User B Shared Name",
                    "category": "Community",
                    "handleOrUrl": format!("{}/shared.xml", server.uri())
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(sub_a["id"], sub_b["id"]);
    assert_ne!(sub_a["subscriptionId"], sub_b["subscriptionId"]);
    let shared_id: Uuid = sub_a["id"].as_str().unwrap().parse().unwrap();

    // User A creates a user_private source, AI config, and webhook channel.
    let priv_a = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::POST,
                "/api/v1/settings/sources",
                &token_a,
                Some(serde_json::json!({
                    "sourceType": "rss",
                    "scope": "user_private",
                    "canonicalKey": "private-rss-a",
                    "customName": "User A Private Feed",
                    "category": "Private",
                    "handleOrUrl": format!("{}/private-a.xml", server.uri())
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let priv_a_id: Uuid = priv_a["id"].as_str().unwrap().parse().unwrap();
    assert_ne!(priv_a_id, shared_id);

    let ai_a = harness
        .app
        .clone()
        .oneshot(authed_req(
            Method::PUT,
            "/api/v1/settings/ai",
            &token_a,
            Some(serde_json::json!({
                "providerLabel": "UserA-AI",
                "baseUrl": "https://93.184.216.34/v1",
                "apiKey": "test-user-a-secret-key",
                "model": "deepseek-chat"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(ai_a.status(), StatusCode::OK);

    let chan_a = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::POST,
                "/api/v1/settings/channels",
                &token_a,
                Some(serde_json::json!({
                    "channelType": "webhook",
                    "name": "User A Hook",
                    "config": { "url": format!("{}/hook-a", server.uri()) }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let chan_a_id = chan_a["id"].as_str().unwrap().to_owned();

    // Poll shared source once + User A composes digest inline (which also polls User A's private source once, while shared source hits freshness TTL).
    let due_sources = harness
        .repos
        .sources
        .list_pollable_sources(None, Utc::now(), 10)
        .await
        .unwrap();
    assert_eq!(due_sources.len(), 2);

    let shared_rec = due_sources.iter().find(|s| s.id == shared_id).unwrap();
    let shared_poll = poll_canonical_source_once(
        &harness.repos,
        &harness.deps.cache,
        &harness.deps.cipher,
        &harness.deps.collectors,
        &harness.deps.pipeline,
        &harness.deps.config.core.collector,
        &harness.deps.config.core.resources,
        shared_rec,
        false,
    )
    .await
    .unwrap();
    assert_eq!(shared_poll.items_fetched, 1);

    let digest_date = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
    let outcome_a = harness
        .orchestrator
        .compose_user_digest_inline(user_a_id, digest_date, RunType::ManualDaily, false, true)
        .await
        .unwrap();
    assert_eq!(outcome_a.deliveries.len(), 1);
    let delivery_a_id = outcome_a.deliveries[0].id;

    // Both User A and User B see the shared_public items.
    let shared_items_b = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::GET,
                &format!("/api/v1/sources/{shared_id}/items"),
                &token_b,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(shared_items_b["total"], 1);
    assert_eq!(
        shared_items_b["items"][0]["title"],
        "SharedItem Announcement"
    );

    // User B CANNOT see or mutate ANY of User A's private sources, items, AI config, channels, digests, runs, or deliveries.
    let sources_b = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::GET,
                "/api/v1/settings/sources",
                &token_b,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(sources_b.as_array().unwrap().len(), 1);
    assert_eq!(sources_b[0]["id"], shared_id.to_string());

    for (m, uri, body) in [
        (
            Method::GET,
            format!("/api/v1/sources/{priv_a_id}/items"),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/sources/{priv_a_id}/digests"),
            None,
        ),
        (
            Method::PUT,
            format!("/api/v1/settings/sources/{priv_a_id}"),
            Some(serde_json::json!({ "enabled": false })),
        ),
        (
            Method::DELETE,
            format!("/api/v1/settings/sources/{priv_a_id}"),
            None,
        ),
        (
            Method::POST,
            format!("/api/v1/settings/sources/{priv_a_id}/test"),
            None,
        ),
        (
            Method::PUT,
            format!("/api/v1/settings/channels/{chan_a_id}"),
            Some(serde_json::json!({ "name": "Hijack" })),
        ),
        (
            Method::DELETE,
            format!("/api/v1/settings/channels/{chan_a_id}"),
            None,
        ),
        (
            Method::POST,
            format!("/api/v1/settings/channels/{chan_a_id}/test"),
            None,
        ),
        (Method::GET, "/api/v1/digests/2026-11-26".to_owned(), None),
        (
            Method::POST,
            format!("/api/v1/deliveries/{delivery_a_id}/resend"),
            None,
        ),
    ] {
        let resp = harness
            .app
            .clone()
            .oneshot(authed_req(m, &uri, &token_b, body))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "expected 404 isolation for User B on {uri}"
        );
        assert_eq!(read_json(resp).await["code"], ErrorCode::NotFound.as_str());
    }

    let search_priv_a = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::GET,
                "/api/v1/items?q=PrivateOnlyToken",
                &token_a,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(search_priv_a["total"], 1);

    let search_priv_b = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::GET,
                "/api/v1/items?q=PrivateOnlyToken",
                &token_b,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(search_priv_b["total"], 0);

    let ai_b = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::GET,
                "/api/v1/settings/ai",
                &token_b,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(ai_b["hasApiKey"], false);
    assert_ne!(ai_b["providerLabel"], "UserA-AI");

    let runs_b = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(Method::GET, "/api/v1/runs", &token_b, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(runs_b["total"], 0);

    // Repo-level guard: shared_public config cannot be mutated per-user.
    let shared_mut_err = harness
        .repos
        .sources
        .update_user_subscription(
            user_b_id,
            shared_id,
            UpdateSourceCmd {
                handle_or_url: Some("https://example.com/hijack.xml".to_owned()),
                ..UpdateSourceCmd::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(shared_mut_err, CoreError::Forbidden(_)));

    // User B unsubscribing from shared_public keeps User A's subscription intact.
    let del_shared_b = harness
        .app
        .clone()
        .oneshot(authed_req(
            Method::DELETE,
            &format!("/api/v1/settings/sources/{shared_id}"),
            &token_b,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_shared_b.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        harness
            .repos
            .sources
            .list_user_sources(user_a_id)
            .await
            .unwrap()
            .len(),
        2
    );

    // User A deleting user_private source removes it cleanly.
    let del_priv_a = harness
        .app
        .oneshot(authed_req(
            Method::DELETE,
            &format!("/api/v1/settings/sources/{priv_a_id}"),
            &token_a,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_priv_a.status(), StatusCode::NO_CONTENT);
    assert!(harness
        .repos
        .sources
        .get_user_source(user_a_id, priv_a_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn retention_prunes_180_day_unbookmarked_bodies_preserves_bookmarked_and_excludes_from_fts() {
    let server = MockServer::start().await;
    let (encoding_key, jwk) = build_rsa_keypair_and_jwk("iso-key-1");
    let issuer = format!("{}/issuer", server.uri());

    Mock::given(method("GET"))
        .and(path("/issuer/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer": issuer,
            "jwks_uri": format!("{}/jwks.json", server.uri()),
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "keys": [jwk]
        })))
        .mount(&server)
        .await;

    let harness = build_oidc_harness(issuer.clone());
    let token = mint_token(&encoding_key, &issuer, "retention-user", "ret@example.com");

    let src = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::POST,
                "/api/v1/settings/sources",
                &token,
                Some(serde_json::json!({
                    "sourceType": "rss",
                    "canonicalKey": "retention-rss",
                    "customName": "Retention Feed",
                    "category": "Official",
                    "handleOrUrl": "https://93.184.216.34/feed.xml"
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let source_id: Uuid = src["id"].as_str().unwrap().parse().unwrap();

    let inserted = harness
        .repos
        .sources
        .upsert_canonical_items(
            source_id,
            &[
                NewCanonicalItem {
                    external_id: Some("bm-1".to_owned()),
                    canonical_url: "https://example.com/ret/1".to_owned(),
                    content_hash: "ret-hash-1".to_owned(),
                    title: "Bookmarked Title".to_owned(),
                    author: None,
                    summary_raw: Some("unique_bookmarked_token_xyz".to_owned()),
                    published_at: Some(Utc::now()),
                },
                NewCanonicalItem {
                    external_id: Some("unbm-2".to_owned()),
                    canonical_url: "https://example.com/ret/2".to_owned(),
                    content_hash: "ret-hash-2".to_owned(),
                    title: "Unbookmarked Title".to_owned(),
                    author: None,
                    summary_raw: Some("unique_unbookmarked_token_abc".to_owned()),
                    published_at: Some(Utc::now()),
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(inserted.len(), 2);

    // Bookmark item #1 via REST API.
    let bm_resp = harness
        .app
        .clone()
        .oneshot(authed_req(
            Method::POST,
            "/api/v1/bookmarks",
            &token,
            Some(serde_json::json!({ "itemId": inserted[0].id, "note": "Keep summary" })),
        ))
        .await
        .unwrap();
    assert_eq!(bm_resp.status(), StatusCode::OK);

    // Before 180 days: both tokens are searchable and 0 items pruned.
    let before_unbm = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::GET,
                "/api/v1/items?q=unique_unbookmarked_token_abc",
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(before_unbm["total"], 1);
    assert_eq!(
        run_retention_once(&harness.repos, 180, Utc::now())
            .await
            .unwrap(),
        0
    );

    // Advance evaluation clock past 180 days: unbookmarked body is pruned, bookmarked body preserved.
    let pruned = run_retention_once(&harness.repos, 180, Utc::now() + ChronoDuration::days(181))
        .await
        .unwrap();
    assert_eq!(pruned, 1);

    let items_json = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::GET,
                &format!("/api/v1/sources/{source_id}/items"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let items_arr = items_json["items"].as_array().unwrap();
    let bm_item = items_arr
        .iter()
        .find(|it| it["id"] == inserted[0].id.to_string())
        .unwrap();
    let unbm_item = items_arr
        .iter()
        .find(|it| it["id"] == inserted[1].id.to_string())
        .unwrap();

    assert_eq!(bm_item["bodyPruned"], false);
    assert_eq!(bm_item["summary"], "unique_bookmarked_token_xyz");
    assert_eq!(unbm_item["bodyPruned"], true);
    assert!(unbm_item["summary"].is_null());

    // Pruned body is excluded from full-text search, while bookmarked body stays searchable.
    let after_bm = read_json(
        harness
            .app
            .clone()
            .oneshot(authed_req(
                Method::GET,
                "/api/v1/items?q=unique_bookmarked_token_xyz",
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(after_bm["total"], 1);

    let after_unbm = read_json(
        harness
            .app
            .oneshot(authed_req(
                Method::GET,
                "/api/v1/items?q=unique_unbookmarked_token_abc",
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(after_unbm["total"], 0);
}
