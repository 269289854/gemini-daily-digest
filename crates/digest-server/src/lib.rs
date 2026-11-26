//! `digest-server` library crate exposing configuration, repository abstractions,
//! auth/SSE/execution services, scheduler, and Axum router builder.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod api;
pub mod auth;
pub mod config;
pub mod constants;
pub mod repo;
pub mod scheduler;
pub mod services;
pub mod sse_hub;
pub mod state;

use axum::Router;
use digest_core::constants::HEADER_REQUEST_ID;
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use http::{HeaderName, Method};
use tower_http::cors::{Any, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// Builds the complete Axum router with request-id propagation, tracing, and CORS layers.
pub fn build_router(state: AppState) -> Router {
    let request_id_header = HeaderName::from_static(HEADER_REQUEST_ID);
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([AUTHORIZATION, CONTENT_TYPE, request_id_header.clone()])
        .expose_headers([request_id_header.clone()]);

    api::router(state)
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(TraceLayer::new_for_http())
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
        .layer(cors)
}
