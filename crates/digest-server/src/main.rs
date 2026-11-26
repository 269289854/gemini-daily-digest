//! Axum HTTP/SSE server and multi-user scheduler entrypoint.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use axum::{routing::get, Json, Router};
use digest_core::config::AppConfig;
use digest_core::constants::{HEALTH_STATUS_OK, ROUTE_HEALTH, SERVICE_NAME};
use serde::Serialize;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

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

/// Builds the Axum router (extended with OIDC + REST + SSE routes in Round 2).
pub fn build_router() -> Router {
    Router::new().route(ROUTE_HEALTH, get(health_handler))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = AppConfig::from_env()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.resources.tokio_worker_threads.get())
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        let listener = TcpListener::bind(&config.server_bind_addr).await?;
        tracing::info!(
            bind_addr = %config.server_bind_addr,
            nio_workers = config.resources.tokio_worker_threads.get(),
            "digest-server listening"
        );
        axum::serve(listener, build_router()).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn health_endpoint_returns_ok_json() {
        let app = build_router();
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
    }
}
