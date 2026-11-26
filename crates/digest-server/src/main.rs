//! Axum HTTP/SSE server and multi-user scheduler entrypoint.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::net::SocketAddr;
use std::sync::Arc;

use digest_core::cache::build_cache_coordinator;
use digest_core::collectors::CollectorRegistry;
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
use digest_server::repo::postgres::{connect_pg_pool, run_migrations, PostgresRepoStore};
use digest_server::scheduler::{spawn_scheduler_tasks, ComposerDeps, ServerExecutionOrchestrator};
use digest_server::services::{AuthService, ExecutionOrchestrator, SsePublisher};
use digest_server::sse_hub::SseHub;
use digest_server::state::AppState;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

async fn build_runtime_services(
    config: ServerConfig,
) -> Result<(AppState, ComposerDeps), CoreError> {
    config.validate()?;

    let cipher = Arc::new(config.build_cipher_or_dev_fallback()?);
    let ssrf = SsrfGuard::from_config(&config.core);
    let http = shared_http_client(
        config.core.outbound_proxy_url.as_deref(),
        &config.core.collector,
    )?;
    let cache = build_cache_coordinator(&config.core).await?;

    let repos = match config.database_url.as_deref() {
        Some(database_url) => {
            let pool =
                connect_pg_pool(database_url, config.core.resources.pg_max_connections).await?;
            run_migrations(&pool).await?;
            tracing::info!(
                storage = "postgres",
                max_connections = config.core.resources.pg_max_connections,
                "PostgreSQL repository store initialized and migrated"
            );
            PostgresRepoStore::with_cipher(pool, Arc::clone(&cipher)).into_repositories()
        }
        None => {
            tracing::warn!(
                storage = "in_memory",
                "DATABASE_URL unset; running with ephemeral in-memory repository store"
            );
            InMemoryRepoStore::repositories_with_cipher(Arc::clone(&cipher))
        }
    };

    let config = Arc::new(config);
    let auth: Arc<dyn AuthService> = Arc::new(HybridAuthService::new(
        Arc::clone(&config),
        Arc::clone(&repos.users),
        http.clone(),
    ));
    let sse: Arc<dyn SsePublisher> = Arc::new(SseHub::new());
    let collectors = Arc::new(CollectorRegistry::from_config(&config.core)?);
    let pipeline = Arc::new(ItemProcessingPipeline::default());
    let llm_provider: Arc<dyn StructuredLlmProvider> =
        Arc::new(OpenAiCompatProvider::from_config(&config.core)?);
    let llm_runner = Arc::new(IdempotentLlmRunner::new(
        Arc::clone(&cache),
        Arc::clone(&llm_provider),
    ));
    let renderer = Arc::new(RendererRegistry::new());
    let notifiers = Arc::new(NotifierRegistry::from_config(&config.core)?);

    let composer_deps = ComposerDeps {
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
        composer_deps.clone(),
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

    Ok((state, composer_deps))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %err, "failed to install Ctrl+C signal handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to install SIGTERM signal handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = ServerConfig::from_env()?;
    config.validate()?;

    let worker_threads = config.core.resources.tokio_worker_threads.get();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        let (state, composer_deps) = build_runtime_services(config).await?;
        let bind_addr = state.config.core.server_bind_addr.clone();
        let auth_mode = state.config.core.auth_mode;
        let router = build_router(state);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let scheduler_handles = spawn_scheduler_tasks(composer_deps, shutdown_rx);

        let listener = TcpListener::bind(&bind_addr).await?;
        tracing::info!(
            bind_addr = %bind_addr,
            auth_mode = %auth_mode,
            nio_workers = worker_threads,
            "digest-server listening"
        );

        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await?;

        tracing::info!("shutdown signal received; stopping scheduler loops");
        let _ = shutdown_tx.send(true);
        scheduler_handles.join_all().await;
        tracing::info!("digest-server shutdown complete");

        Ok::<(), Box<dyn std::error::Error>>(())
    })
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use digest_core::constants::{API_PREFIX, HEALTH_STATUS_OK, ROUTE_HEALTH, SERVICE_NAME};
    use digest_server::constants::{DEV_BYPASS_OIDC_SUB, SUBROUTE_AUTH_CONFIG, SUBROUTE_ME};
    use http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn health_endpoint_returns_ok_json() {
        let (state, _deps) = build_runtime_services(ServerConfig::default())
            .await
            .unwrap();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(ROUTE_HEALTH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["status"], HEALTH_STATUS_OK);
        assert_eq!(json["service"], SERVICE_NAME);
    }

    #[tokio::test]
    async fn main_wiring_populates_routes_and_shuts_down_scheduler_cleanly() {
        let (state, deps) = build_runtime_services(ServerConfig::default())
            .await
            .unwrap();
        let app = build_router(state);

        let auth_cfg_uri = format!("{API_PREFIX}{SUBROUTE_AUTH_CONFIG}");
        let auth_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&auth_cfg_uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(auth_resp.status(), StatusCode::OK);

        let me_uri = format!("{API_PREFIX}{SUBROUTE_ME}");
        let mut me_req = Request::builder().uri(&me_uri).body(Body::empty()).unwrap();
        me_req.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            55555,
        )));
        let me_resp = app.oneshot(me_req).await.unwrap();
        assert_eq!(me_resp.status(), StatusCode::OK);
        let bytes = me_resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["user"]["oidcSub"], DEV_BYPASS_OIDC_SUB);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handles = spawn_scheduler_tasks(deps, shutdown_rx);
        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), handles.join_all())
            .await
            .unwrap();
    }
}
