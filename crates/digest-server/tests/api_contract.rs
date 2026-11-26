#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::Router;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chrono::Utc;
use digest_core::cache::{CacheCoordinator, InMemoryCacheCoordinator};
use digest_core::collectors::{CollectorContext, CollectorRegistry};
use digest_core::config::{AppConfig, CollectorTuning, LlmTuning, NotifierTuning};
use digest_core::constants::{
    CRYPTO_KEY_BYTE_LEN, HEADER_AUTHORIZATION, HEADER_BEARER_PREFIX, HEADER_CONTENT_TYPE,
    HEADER_REQUEST_ID, HEALTH_STATUS_OK, ROUTE_HEALTH, SERVICE_NAME,
};
use digest_core::domain::{AuthMode, ErrorCode, RunStatus};
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
use digest_server::constants::{CONTENT_TYPE_MARKDOWN_UTF8, DEV_BYPASS_OIDC_SUB};
use digest_server::repo::in_memory::InMemoryRepoStore;
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

fn build_test_app(
    auth_mode: AuthMode,
    oidc_issuer_url: Option<String>,
    oidc_client_id: Option<String>,
    allow_loopback_ssrf: bool,
) -> (Router, AppState) {
    let mut core = AppConfig {
        auth_mode,
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
    if allow_loopback_ssrf {
        core.ssrf_allowlist_hosts = vec![
            "127.0.0.1".to_owned(),
            "localhost".to_owned(),
            "blog.google".to_owned(),
        ];
    }

    let config = Arc::new(ServerConfig {
        core,
        database_url: None,
        oidc_issuer_url,
        oidc_client_id,
        oidc_audience: None,
        app_master_key_b64: Some(STANDARD.encode([0x33u8; CRYPTO_KEY_BYTE_LEN])),
        ..ServerConfig::default()
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
    let executor: Arc<dyn ExecutionOrchestrator> = Arc::new(ServerExecutionOrchestrator::new(
        deps,
        ssrf.clone(),
        llm_provider,
    ));

    let state = AppState {
        config,
        repos,
        cipher,
        ssrf,
        cache,
        auth,
        sse,
        executor,
        http,
    };

    let router = build_router(state.clone());
    (router, state)
}

fn json_request(
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
    peer_ip: Option<IpAddr>,
    bearer: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        builder = builder.header(
            HEADER_AUTHORIZATION,
            format!("{HEADER_BEARER_PREFIX}{token}"),
        );
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(HEADER_CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    let mut req = builder.body(body).unwrap();
    if let Some(ip) = peer_ip {
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(ip, 51234)));
    }
    req
}

async fn read_json_body(resp: http::Response<Body>) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn assert_error_envelope(json: &serde_json::Value, expected_code: ErrorCode) {
    assert_eq!(json["code"], expected_code.as_str());
    assert!(json["message"].as_str().is_some_and(|m| !m.is_empty()));
    assert!(json["requestId"].as_str().is_some_and(|id| !id.is_empty()));
}

#[tokio::test]
async fn oidc_rs256_jwks_and_dev_bypass_enforce_strict_auth_boundaries() {
    let oidc_server = MockServer::start().await;
    let (encoding_key, jwk) = build_rsa_keypair_and_jwk("key-contract-1");
    let issuer = format!("{}/issuer", oidc_server.uri());
    let jwks_uri = format!("{}/jwks.json", oidc_server.uri());

    Mock::given(method("GET"))
        .and(path("/issuer/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer": issuer,
            "jwks_uri": jwks_uri,
        })))
        .mount(&oidc_server)
        .await;

    Mock::given(method("GET"))
        .and(path("/jwks.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "keys": [jwk]
        })))
        .mount(&oidc_server)
        .await;

    let (oidc_app, _) = build_test_app(
        AuthMode::Oidc,
        Some(issuer.clone()),
        Some("digest-web-client".to_owned()),
        false,
    );

    // Public auth config works unauthenticated.
    let cfg_resp = oidc_app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/auth/config",
            None,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(cfg_resp.status(), StatusCode::OK);
    let cfg_json = read_json_body(cfg_resp).await;
    assert_eq!(cfg_json["authMode"], "oidc");
    assert_eq!(cfg_json["issuerUrl"], issuer);
    assert_eq!(cfg_json["clientId"], "digest-web-client");
    assert_eq!(
        cfg_json["scopes"],
        serde_json::json!(["openid", "profile", "email", "offline_access"])
    );

    let now = usize::try_from(Utc::now().timestamp()).unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("key-contract-1".to_owned());

    // Valid RS256 JWT succeeds.
    let valid_jwt = jsonwebtoken::encode(
        &header,
        &OidcClaims {
            sub: "oidc-user-alpha".to_owned(),
            iss: issuer.clone(),
            aud: "digest-web-client".to_owned(),
            exp: now + 600,
            nbf: now.saturating_sub(10),
            email: Some("alpha@example.com".to_owned()),
            name: Some("Alpha User".to_owned()),
        },
        &encoding_key,
    )
    .unwrap();

    let ok_me = oidc_app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/me",
            None,
            None,
            Some(&valid_jwt),
        ))
        .await
        .unwrap();
    assert_eq!(ok_me.status(), StatusCode::OK);
    assert!(ok_me.headers().contains_key(HEADER_REQUEST_ID));
    let ok_me_json = read_json_body(ok_me).await;
    assert_eq!(ok_me_json["user"]["oidcSub"], "oidc-user-alpha");
    assert_eq!(ok_me_json["user"]["email"], "alpha@example.com");

    // Expired JWT rejected.
    let expired_jwt = jsonwebtoken::encode(
        &header,
        &OidcClaims {
            sub: "oidc-user-alpha".to_owned(),
            iss: issuer.clone(),
            aud: "digest-web-client".to_owned(),
            exp: now.saturating_sub(300),
            nbf: now.saturating_sub(600),
            email: Some("alpha@example.com".to_owned()),
            name: None,
        },
        &encoding_key,
    )
    .unwrap();
    let expired_resp = oidc_app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/me",
            None,
            None,
            Some(&expired_jwt),
        ))
        .await
        .unwrap();
    assert_eq!(expired_resp.status(), StatusCode::UNAUTHORIZED);
    assert_error_envelope(&read_json_body(expired_resp).await, ErrorCode::Unauthorized);

    // Wrong audience JWT rejected.
    let wrong_aud_jwt = jsonwebtoken::encode(
        &header,
        &OidcClaims {
            sub: "oidc-user-alpha".to_owned(),
            iss: issuer.clone(),
            aud: "other-client".to_owned(),
            exp: now + 600,
            nbf: now.saturating_sub(10),
            email: Some("alpha@example.com".to_owned()),
            name: None,
        },
        &encoding_key,
    )
    .unwrap();
    let wrong_aud_resp = oidc_app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/me",
            None,
            None,
            Some(&wrong_aud_jwt),
        ))
        .await
        .unwrap();
    assert_eq!(wrong_aud_resp.status(), StatusCode::UNAUTHORIZED);
    assert_error_envelope(
        &read_json_body(wrong_aud_resp).await,
        ErrorCode::Unauthorized,
    );

    // alg=none unsigned JWT rejected.
    let alg_none_header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let alg_none_payload = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&OidcClaims {
            sub: "oidc-user-alpha".to_owned(),
            iss: issuer,
            aud: "digest-web-client".to_owned(),
            exp: now + 600,
            nbf: now.saturating_sub(10),
            email: Some("alpha@example.com".to_owned()),
            name: None,
        })
        .unwrap(),
    );
    let alg_none_jwt = format!("{alg_none_header}.{alg_none_payload}.");
    let alg_none_resp = oidc_app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/me",
            None,
            None,
            Some(&alg_none_jwt),
        ))
        .await
        .unwrap();
    assert_eq!(alg_none_resp.status(), StatusCode::UNAUTHORIZED);
    assert_error_envelope(
        &read_json_body(alg_none_resp).await,
        ErrorCode::Unauthorized,
    );

    // dev_bypass checks: 127.0.0.1 passes, spoofed X-Forwarded-For on non-loopback fails, missing ConnectInfo fails.
    let (dev_app, _) = build_test_app(AuthMode::DevBypass, None, None, false);

    let loopback_ok = dev_app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/me",
            None,
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(loopback_ok.status(), StatusCode::OK);
    assert_eq!(
        read_json_body(loopback_ok).await["user"]["oidcSub"],
        DEV_BYPASS_OIDC_SUB
    );

    let mut spoofed_remote = json_request(
        Method::GET,
        "/api/v1/me",
        None,
        Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 25))),
        None,
    );
    spoofed_remote
        .headers_mut()
        .insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
    spoofed_remote
        .headers_mut()
        .insert("forwarded", "for=127.0.0.1".parse().unwrap());
    let remote_resp = dev_app.clone().oneshot(spoofed_remote).await.unwrap();
    assert_eq!(remote_resp.status(), StatusCode::FORBIDDEN);
    assert_error_envelope(&read_json_body(remote_resp).await, ErrorCode::Forbidden);

    let missing_conn_resp = dev_app
        .oneshot(json_request(Method::GET, "/api/v1/me", None, None, None))
        .await
        .unwrap();
    assert_eq!(missing_conn_resp.status(), StatusCode::UNAUTHORIZED);
    assert_error_envelope(
        &read_json_body(missing_conn_resp).await,
        ErrorCode::Unauthorized,
    );
}

#[tokio::test]
async fn all_section_5_endpoints_ssrf_masking_and_sse_work_end_to_end() {
    let upstream = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Gemini Ecosystem Feed</title>
    <item>
      <guid>gem-1</guid>
      <title>Gemini 2.5 Pro SDK Release</title>
      <link>https://example.com/posts/gemini-25-pro</link>
      <description>Official Gemini 2.5 Pro SDK release notes and benchmarks.</description>
    </item>
    <item>
      <guid>gem-2</guid>
      <title>Developer Tooling Roundup</title>
      <link>https://example.com/posts/tooling-roundup</link>
      <description>Weekly community tooling tips.</description>
    </item>
  </channel>
</rss>"#,
        ))
        .mount(&upstream)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "deepseek-chat",
            "choices": [{
                "message": {
                    "content": "{\"summaryMarkdown\":\"Gemini 2.5 Pro SDK launched with benchmarks.\",\"keyPoints\":[\"Gemini 2.5 Pro SDK released\",\"Community tooling updates\"]}"
                }
            }],
            "usage": {
                "prompt_tokens": 40,
                "completion_tokens": 18
            }
        })))
        .mount(&upstream)
        .await;

    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200).set_body_string("accepted"))
        .mount(&upstream)
        .await;

    let (app, _state) = build_test_app(
        AuthMode::DevBypass,
        Some("https://auth.example.com/application/o/digest".to_owned()),
        Some("digest-web".to_owned()),
        true,
    );
    let (strict_app, _) = build_test_app(AuthMode::DevBypass, None, None, false);

    let loopback = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));

    // 1. /health, /api/v1/auth/config, /api/v1/me
    let health = app
        .clone()
        .oneshot(json_request(Method::GET, ROUTE_HEALTH, None, None, None))
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    let health_json = read_json_body(health).await;
    assert_eq!(health_json["status"], HEALTH_STATUS_OK);
    assert_eq!(health_json["service"], SERVICE_NAME);

    let auth_cfg = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/auth/config",
            None,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(auth_cfg.status(), StatusCode::OK);
    assert_eq!(read_json_body(auth_cfg).await["authMode"], "dev_bypass");

    let me = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/me",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let me_json = read_json_body(me).await;
    assert_eq!(me_json["onboarding"]["hasAiConfig"], false);

    // 2. AI config GET / PUT / test + SSRF enforcement + secret masking
    let ai_get = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/settings/ai",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(ai_get.status(), StatusCode::OK);
    assert_eq!(read_json_body(ai_get).await["hasApiKey"], false);

    let ssrf_ai_put = strict_app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/settings/ai",
            Some(serde_json::json!({
                "providerLabel": "Local",
                "baseUrl": "http://127.0.0.1:11434/v1",
                "apiKey": "test-ai-secret-value-1111",
                "model": "llama3"
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(ssrf_ai_put.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_error_envelope(&read_json_body(ssrf_ai_put).await, ErrorCode::SsrfBlocked);

    let ai_secret = "test-ai-secret-key-9999";
    let ai_put = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/settings/ai",
            Some(serde_json::json!({
                "providerLabel": "MockLLM",
                "baseUrl": upstream.uri(),
                "apiKey": ai_secret,
                "model": "deepseek-chat",
                "systemPrompt": "Summarize concisely.",
                "temperature": 0.2,
                "maxTokens": 1024
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(ai_put.status(), StatusCode::OK);
    let ai_put_bytes = ai_put.into_body().collect().await.unwrap().to_bytes();
    let ai_put_text = String::from_utf8_lossy(&ai_put_bytes);
    assert!(!ai_put_text.contains(ai_secret));
    let ai_put_json: serde_json::Value = serde_json::from_slice(&ai_put_bytes).unwrap();
    assert_eq!(ai_put_json["hasApiKey"], true);
    assert_eq!(ai_put_json["apiKeyHint"], "test****9999");

    let ai_test_ssrf = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/ai/test",
            Some(serde_json::json!({ "baseUrl": "http://169.254.169.254/v1" })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(ai_test_ssrf.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_error_envelope(&read_json_body(ai_test_ssrf).await, ErrorCode::SsrfBlocked);

    let ai_test_ok = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/ai/test",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(ai_test_ok.status(), StatusCode::OK);
    let ai_test_json = read_json_body(ai_test_ok).await;
    assert_eq!(ai_test_json["ok"], true);
    assert_eq!(ai_test_json["model"], "deepseek-chat");

    // 3. Sources CRUD + preset + export + import + test + SSRF block
    let ssrf_src = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/sources",
            Some(serde_json::json!({
                "sourceType": "rss",
                "handleOrUrl": "http://169.254.169.254/feed.xml"
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(ssrf_src.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_error_envelope(&read_json_body(ssrf_src).await, ErrorCode::SsrfBlocked);

    let feed_url = format!("{}/feed.xml", upstream.uri());
    let create_rss = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/sources",
            Some(serde_json::json!({
                "sourceType": "rss",
                "scope": "shared_public",
                "canonicalKey": "contract-mock-rss",
                "customName": "Mock Official Feed",
                "category": "Official",
                "handleOrUrl": feed_url,
                "enabled": true
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(create_rss.status(), StatusCode::OK);
    let rss_json = read_json_body(create_rss).await;
    let rss_source_id = rss_json["id"].as_str().unwrap().to_owned();
    assert_eq!(rss_json["scope"], "shared_public");

    let x_secret = "test-x-bearer-secret-7788";
    let create_priv = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/sources",
            Some(serde_json::json!({
                "sourceType": "x_account",
                "customName": "Private X Account",
                "category": "X",
                "handleOrUrl": "GeminiExample",
                "credentials": { "bearerToken": x_secret },
                "enabled": true
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(create_priv.status(), StatusCode::OK);
    let priv_bytes = create_priv.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&priv_bytes).contains(x_secret));
    let priv_json: serde_json::Value = serde_json::from_slice(&priv_bytes).unwrap();
    assert_eq!(priv_json["scope"], "user_private");
    assert_eq!(priv_json["hasCredentials"], true);
    assert_eq!(priv_json["configHint"], "test****7788");
    let priv_id = priv_json["id"].as_str().unwrap().to_owned();

    let upd_priv = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &format!("/api/v1/settings/sources/{priv_id}"),
            Some(serde_json::json!({ "customName": "Paused Private X", "enabled": false })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(upd_priv.status(), StatusCode::OK);
    assert_eq!(read_json_body(upd_priv).await["enabled"], false);

    let del_priv = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            &format!("/api/v1/settings/sources/{priv_id}"),
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_priv.status(), StatusCode::NO_CONTENT);

    // Preset import, export, import, then disable/delete preset so manual run stays offline.
    let preset = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/sources/import-preset",
            Some(serde_json::json!({ "canonicalKeys": ["google-ai-blog"] })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(preset.status(), StatusCode::OK);
    let preset_json = read_json_body(preset).await;
    assert_eq!(preset_json["importedCount"], 1);
    let preset_id = preset_json["sources"][0]["id"].as_str().unwrap().to_owned();

    let exported = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/sources/export",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(exported.status(), StatusCode::OK);
    let exported_bytes = exported.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&exported_bytes).contains(x_secret));
    let exported_json: serde_json::Value = serde_json::from_slice(&exported_bytes).unwrap();
    assert_eq!(exported_json["version"], 1);

    let reimport = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/sources/import",
            Some(exported_json),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(reimport.status(), StatusCode::OK);

    let del_preset = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            &format!("/api/v1/settings/sources/{preset_id}"),
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_preset.status(), StatusCode::NO_CONTENT);

    let list_src = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/settings/sources",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(list_src.status(), StatusCode::OK);
    assert_eq!(read_json_body(list_src).await.as_array().unwrap().len(), 1);

    let test_src = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/settings/sources/{rss_source_id}/test"),
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(test_src.status(), StatusCode::OK);
    let test_src_json = read_json_body(test_src).await;
    assert_eq!(test_src_json["outcome"], "updated");
    assert_eq!(test_src_json["itemsFetched"], 2);
    assert_eq!(
        test_src_json["sampleTitles"][0],
        "Gemini 2.5 Pro SDK Release"
    );

    // 4. Notification channels CRUD + test + SSRF + masking
    let ssrf_chan = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/channels",
            Some(serde_json::json!({
                "channelType": "webhook",
                "name": "Metadata Hook",
                "config": { "url": "http://169.254.169.254/hook" }
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(ssrf_chan.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_error_envelope(&read_json_body(ssrf_chan).await, ErrorCode::SsrfBlocked);

    let hook_secret = "test-hook-bearer-secret-4321";
    let hook_url = format!("{}/hook", upstream.uri());
    let create_chan = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/channels",
            Some(serde_json::json!({
                "channelType": "webhook",
                "name": "Mock Webhook",
                "enabled": true,
                "config": {
                    "url": hook_url,
                    "headers": { "X-Hook-Secret": hook_secret }
                },
                "triggerOnDaily": true,
                "triggerOnAlert": true
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(create_chan.status(), StatusCode::OK);
    let chan_bytes = create_chan.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&chan_bytes).contains(hook_secret));
    let chan_json: serde_json::Value = serde_json::from_slice(&chan_bytes).unwrap();
    let channel_id = chan_json["id"].as_str().unwrap().to_owned();
    assert!(chan_json["configHint"].as_str().is_some());

    let list_chan = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/settings/channels",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(list_chan.status(), StatusCode::OK);
    let list_chan_bytes = list_chan.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&list_chan_bytes).contains(hook_secret));

    let upd_chan = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &format!("/api/v1/settings/channels/{channel_id}"),
            Some(serde_json::json!({ "name": "Renamed Mock Webhook" })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(upd_chan.status(), StatusCode::OK);
    assert_eq!(
        read_json_body(upd_chan).await["name"],
        "Renamed Mock Webhook"
    );

    let test_chan = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/settings/channels/{channel_id}/test"),
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(test_chan.status(), StatusCode::OK);
    assert_eq!(read_json_body(test_chan).await["status"], "sent");

    // Create + delete temp channel to cover DELETE /settings/channels/{id}
    let temp_chan = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/channels",
            Some(serde_json::json!({
                "channelType": "webhook",
                "name": "Temp Hook",
                "config": { "url": format!("{}/hook", upstream.uri()) }
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    let temp_chan_id = read_json_body(temp_chan).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let del_chan = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            &format!("/api/v1/settings/channels/{temp_chan_id}"),
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_chan.status(), StatusCode::NO_CONTENT);

    // 5. Schedule & Watchlist CRUD
    let get_sched = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/settings/schedule",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(get_sched.status(), StatusCode::OK);

    let bad_sched = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/settings/schedule",
            Some(serde_json::json!({
                "timezone": "Bad/Timezone",
                "dailyTime": "09:00",
                "emailEnabled": true,
                "sendWhenEmpty": false,
                "alertScanEnabled": true,
                "alertIntervalMinutes": 60
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bad_sched.status(), StatusCode::BAD_REQUEST);
    assert_error_envelope(&read_json_body(bad_sched).await, ErrorCode::InvalidInput);

    let ok_sched = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/settings/schedule",
            Some(serde_json::json!({
                "timezone": "Asia/Shanghai",
                "dailyTime": "09:00",
                "emailEnabled": true,
                "sendWhenEmpty": false,
                "alertScanEnabled": true,
                "alertIntervalMinutes": 30
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(ok_sched.status(), StatusCode::OK);

    let wl_create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/watchlist",
            Some(serde_json::json!({
                "name": "Gemini 2.5",
                "keywords": ["Gemini 2.5"],
                "excludeKeywords": [],
                "priority": "urgent",
                "notifyImmediately": true,
                "enabled": true
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(wl_create.status(), StatusCode::OK);
    let wl_id = read_json_body(wl_create).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let wl_upd = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &format!("/api/v1/settings/watchlist/{wl_id}"),
            Some(serde_json::json!({
                "name": "Gemini 2.5 Pro",
                "keywords": ["Gemini 2.5"],
                "excludeKeywords": ["rumor"],
                "priority": "urgent",
                "notifyImmediately": true,
                "enabled": true
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(wl_upd.status(), StatusCode::OK);

    let wl_temp = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/settings/watchlist",
            Some(serde_json::json!({
                "name": "Temp Rule",
                "keywords": ["temp"],
                "excludeKeywords": [],
                "priority": "normal",
                "notifyImmediately": false,
                "enabled": false
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    let wl_temp_id = read_json_body(wl_temp).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let wl_del = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            &format!("/api/v1/settings/watchlist/{wl_temp_id}"),
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(wl_del.status(), StatusCode::NO_CONTENT);

    let wl_list = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/settings/watchlist",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(wl_list.status(), StatusCode::OK);
    assert_eq!(read_json_body(wl_list).await.as_array().unwrap().len(), 1);

    // 6. Open SSE stream, trigger manual run, await digest_ready SSE event
    let sse_resp = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/events/stream",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(sse_resp.status(), StatusCode::OK);
    let mut sse_body = sse_resp.into_body();

    let trigger_resp = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runs/trigger",
            Some(serde_json::json!({
                "digestDate": "2026-11-26",
                "forceRefresh": false,
                "notify": true
            })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(trigger_resp.status(), StatusCode::OK);
    let trigger_json = read_json_body(trigger_resp).await;
    assert_eq!(trigger_json["status"], "running");

    let mut sse_acc = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), sse_body.frame()).await {
            Ok(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    sse_acc.push_str(&String::from_utf8_lossy(data));
                    if sse_acc.contains("event: digest_ready") {
                        break;
                    }
                }
            }
            _ => continue,
        }
    }
    assert!(
        sse_acc.contains("event: run_progress"),
        "expected run_progress in SSE stream, got:\n{sse_acc}"
    );
    assert!(
        sse_acc.contains("event: digest_ready"),
        "expected digest_ready in SSE stream, got:\n{sse_acc}"
    );
    assert!(!sse_acc.contains(ai_secret));
    assert!(!sse_acc.contains(hook_secret));
    assert!(!sse_acc.contains(x_secret));

    // Poll /api/v1/runs until status is terminal (succeeded)
    let mut runs_json = serde_json::Value::Null;
    for _ in 0..20 {
        let runs_resp = app
            .clone()
            .oneshot(json_request(
                Method::GET,
                "/api/v1/runs",
                None,
                loopback,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(runs_resp.status(), StatusCode::OK);
        runs_json = read_json_body(runs_resp).await;
        if runs_json["items"][0]["status"] == serde_json::to_value(RunStatus::Succeeded).unwrap() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(runs_json["items"][0]["status"], "succeeded");
    let delivery_id = runs_json["items"][0]["deliveries"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let stats_resp = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/runs/stats",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(stats_resp.status(), StatusCode::OK);
    let stats_json = read_json_body(stats_resp).await;
    assert!(stats_json["totalRuns"].as_u64().unwrap() >= 1);
    assert!(stats_json["totalPromptTokens"].as_u64().unwrap() > 0);
    assert!(stats_json["deliveriesSent"].as_u64().unwrap() >= 1);

    let resend_resp = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/deliveries/{delivery_id}/resend"),
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resend_resp.status(), StatusCode::OK);
    assert_eq!(read_json_body(resend_resp).await["status"], "sent");

    // 7. Digests, Markdown, Source digests/items, Search, Bookmarks
    let list_digests = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/digests",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(list_digests.status(), StatusCode::OK);
    assert_eq!(
        read_json_body(list_digests).await["items"][0]["itemCount"],
        2
    );

    let get_digest = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/digests/2026-11-26",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(get_digest.status(), StatusCode::OK);
    let digest_json = read_json_body(get_digest).await;
    assert_eq!(digest_json["digestDate"], "2026-11-26");
    assert_eq!(digest_json["highlightedCount"], 1);

    let get_md = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/digests/2026-11-26/markdown",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(get_md.status(), StatusCode::OK);
    assert_eq!(
        get_md
            .headers()
            .get(HEADER_CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some(CONTENT_TYPE_MARKDOWN_UTF8)
    );
    let md_body = get_md.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&md_body).contains("Gemini 2.5 Pro SDK"));

    let src_digests = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            &format!("/api/v1/sources/{rss_source_id}/digests"),
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(src_digests.status(), StatusCode::OK);
    assert_eq!(read_json_body(src_digests).await["total"], 1);

    let src_items = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            &format!("/api/v1/sources/{rss_source_id}/items"),
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(src_items.status(), StatusCode::OK);
    let src_items_json = read_json_body(src_items).await;
    assert_eq!(src_items_json["total"], 2);
    let first_item_id: Uuid = src_items_json["items"][0]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let search_items = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/items?q=Gemini",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(search_items.status(), StatusCode::OK);
    assert_eq!(read_json_body(search_items).await["total"], 1);

    let add_bm = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/bookmarks",
            Some(serde_json::json!({ "itemId": first_item_id, "note": "Important SDK" })),
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(add_bm.status(), StatusCode::OK);

    let bm_search = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/items?bookmarked=true",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bm_search.status(), StatusCode::OK);
    let bm_search_json = read_json_body(bm_search).await;
    assert_eq!(bm_search_json["total"], 1);
    assert_eq!(bm_search_json["items"][0]["bookmarked"], true);
    assert_eq!(bm_search_json["items"][0]["bookmarkNote"], "Important SDK");

    let del_bm = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            &format!("/api/v1/bookmarks/{first_item_id}"),
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del_bm.status(), StatusCode::NO_CONTENT);

    // Error envelopes check (404 & 400)
    let missing_digest = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/v1/digests/2026-01-01",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(missing_digest.status(), StatusCode::NOT_FOUND);
    assert_error_envelope(&read_json_body(missing_digest).await, ErrorCode::NotFound);

    let bad_dates = app
        .oneshot(json_request(
            Method::GET,
            "/api/v1/items?dateFrom=2026-12-01&dateTo=2026-11-01",
            None,
            loopback,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bad_dates.status(), StatusCode::BAD_REQUEST);
    assert_error_envelope(&read_json_body(bad_dates).await, ErrorCode::InvalidInput);
}
