//! Pluggable cache and singleflight lock SPI (`Redis` or `InMemory` fallback).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::error::CoreError;

/// Strategy trait abstracting distributed locks and source freshness caching.
#[async_trait]
pub trait CacheCoordinator: Send + Sync {
    /// Attempts to acquire an exclusive lock for `key` with lease `ttl`.
    async fn try_acquire_lock(&self, key: &str, ttl: Duration) -> Result<bool, CoreError>;

    /// Releases an active lock for `key`.
    async fn release_lock(&self, key: &str) -> Result<(), CoreError>;

    /// Returns `true` if `key` is currently marked fresh within its TTL.
    async fn is_fresh(&self, key: &str) -> Result<bool, CoreError>;

    /// Marks `key` as fresh for `ttl`.
    async fn mark_fresh(&self, key: &str, ttl: Duration) -> Result<(), CoreError>;
}

#[derive(Default)]
struct InMemoryState {
    locks: HashMap<String, Instant>,
    fresh_until: HashMap<String, Instant>,
}

/// Zero-dependency in-process coordinator used in tests and single-node setups without Redis.
#[derive(Default)]
pub struct InMemoryCacheCoordinator {
    state: Mutex<InMemoryState>,
}

impl InMemoryCacheCoordinator {
    /// Creates an empty in-memory cache coordinator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl CacheCoordinator for InMemoryCacheCoordinator {
    async fn try_acquire_lock(&self, key: &str, ttl: Duration) -> Result<bool, CoreError> {
        let now = Instant::now();
        let mut guard = self.state.lock().await;
        guard.locks.retain(|_, expires_at| *expires_at > now);
        if guard.locks.contains_key(key) {
            return Ok(false);
        }
        guard.locks.insert(key.to_owned(), now + ttl);
        Ok(true)
    }

    async fn release_lock(&self, key: &str) -> Result<(), CoreError> {
        let mut guard = self.state.lock().await;
        guard.locks.remove(key);
        Ok(())
    }

    async fn is_fresh(&self, key: &str) -> Result<bool, CoreError> {
        let now = Instant::now();
        let mut guard = self.state.lock().await;
        guard.fresh_until.retain(|_, expires_at| *expires_at > now);
        Ok(guard.fresh_until.contains_key(key))
    }

    async fn mark_fresh(&self, key: &str, ttl: Duration) -> Result<(), CoreError> {
        let now = Instant::now();
        let mut guard = self.state.lock().await;
        guard.fresh_until.insert(key.to_owned(), now + ttl);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_coordinator_enforces_lock_exclusivity_and_release() {
        let cache = InMemoryCacheCoordinator::new();
        let ttl = Duration::from_secs(30);

        assert!(cache.try_acquire_lock("src:1", ttl).await.unwrap());
        assert!(!cache.try_acquire_lock("src:1", ttl).await.unwrap());

        cache.release_lock("src:1").await.unwrap();
        assert!(cache.try_acquire_lock("src:1", ttl).await.unwrap());
    }

    #[tokio::test]
    async fn in_memory_coordinator_tracks_freshness_ttl() {
        let cache = InMemoryCacheCoordinator::new();
        assert!(!cache.is_fresh("src:fresh").await.unwrap());

        cache
            .mark_fresh("src:fresh", Duration::from_secs(60))
            .await
            .unwrap();
        assert!(cache.is_fresh("src:fresh").await.unwrap());

        cache
            .mark_fresh("src:expired", Duration::from_millis(1))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(15)).await;
        assert!(!cache.is_fresh("src:expired").await.unwrap());
    }
}
