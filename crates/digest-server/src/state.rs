//! Shared Axum application state injected into handlers, middleware, and workers.

use std::sync::Arc;

use digest_core::cache::CacheCoordinator;
use digest_core::crypto::EnvelopeCipher;
use digest_core::ssrf::SsrfGuard;

use crate::config::ServerConfig;
use crate::repo::Repositories;
use crate::services::{AuthService, ExecutionOrchestrator, SsePublisher};

/// Cloneable application state shared across Axum request handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<ServerConfig>,
    pub repos: Repositories,
    pub cipher: Arc<EnvelopeCipher>,
    pub ssrf: SsrfGuard,
    pub cache: Arc<dyn CacheCoordinator>,
    pub auth: Arc<dyn AuthService>,
    pub sse: Arc<dyn SsePublisher>,
    pub executor: Arc<dyn ExecutionOrchestrator>,
    pub http: reqwest::Client,
}
