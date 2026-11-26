//! Pluggable cache and singleflight lock SPI (`Redis` or `InMemory` fallback).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::constants::{CACHE_FRESH_NAMESPACE, CACHE_LOCK_NAMESPACE, DEFAULT_REDIS_KEY_PREFIX};
use crate::error::CoreError;

const RELEASE_LOCK_LUA: &str = r#"
if redis.call("GET", KEYS[1]) == ARGV[1] then
    return redis.call("DEL", KEYS[1])
else
    return 0
end
"#;

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

    /// Retrieves a cached string value (such as serialized digest output keyed by `input_fingerprint`).
    async fn get_cached_value(&self, _key: &str) -> Result<Option<String>, CoreError> {
        Ok(None)
    }

    /// Stores a string value in cache with an explicit `ttl`.
    async fn set_cached_value(
        &self,
        _key: &str,
        _value: &str,
        _ttl: Duration,
    ) -> Result<(), CoreError> {
        Ok(())
    }
}

#[derive(Default)]
struct InMemoryState {
    locks: HashMap<String, Instant>,
    fresh_until: HashMap<String, Instant>,
    values: HashMap<String, (String, Instant)>,
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

    async fn get_cached_value(&self, key: &str) -> Result<Option<String>, CoreError> {
        let now = Instant::now();
        let mut guard = self.state.lock().await;
        guard.values.retain(|_, (_, expires_at)| *expires_at > now);
        Ok(guard.values.get(key).map(|(value, _)| value.clone()))
    }

    async fn set_cached_value(
        &self,
        key: &str,
        value: &str,
        ttl: Duration,
    ) -> Result<(), CoreError> {
        let now = Instant::now();
        let mut guard = self.state.lock().await;
        guard
            .values
            .insert(key.to_owned(), (value.to_owned(), now + ttl));
        Ok(())
    }
}

/// Redis-backed distributed cache and singleflight lock coordinator using a single
/// multiplexed `redis::aio::ConnectionManager`.
pub struct RedisCacheCoordinator {
    conn: ConnectionManager,
    key_prefix: String,
    lock_tokens: Mutex<HashMap<String, String>>,
}

impl RedisCacheCoordinator {
    /// Connects to Redis and creates a multiplexed `RedisCacheCoordinator`.
    pub async fn connect(
        redis_url: &str,
        key_prefix: impl Into<String>,
    ) -> Result<Self, CoreError> {
        let trimmed_url = redis_url.trim();
        if trimmed_url.is_empty() {
            return Err(CoreError::InvalidConfig(
                "redis_url cannot be empty".to_owned(),
            ));
        }

        let raw_prefix = key_prefix.into();
        let normalized_prefix = if raw_prefix.trim().is_empty() {
            DEFAULT_REDIS_KEY_PREFIX.to_owned()
        } else {
            raw_prefix
        };

        let client = redis::Client::open(trimmed_url)
            .map_err(|err| CoreError::InvalidConfig(format!("invalid redis URL: {err}")))?;
        let conn = ConnectionManager::new(client)
            .await
            .map_err(|err| CoreError::Internal(format!("failed to connect to redis: {err}")))?;

        Ok(Self {
            conn,
            key_prefix: normalized_prefix,
            lock_tokens: Mutex::new(HashMap::new()),
        })
    }

    fn lock_key(&self, key: &str) -> String {
        format!("{}{CACHE_LOCK_NAMESPACE}{key}", self.key_prefix)
    }

    fn fresh_key(&self, key: &str) -> String {
        format!("{}{CACHE_FRESH_NAMESPACE}{key}", self.key_prefix)
    }

    fn value_key(&self, key: &str) -> String {
        format!("{}{key}", self.key_prefix)
    }

    fn ttl_secs(ttl: Duration) -> u64 {
        ttl.as_secs().max(1)
    }
}

#[async_trait]
impl CacheCoordinator for RedisCacheCoordinator {
    async fn try_acquire_lock(&self, key: &str, ttl: Duration) -> Result<bool, CoreError> {
        let redis_key = self.lock_key(key);
        let owner_token = Uuid::new_v4().to_string();
        let ttl_secs = Self::ttl_secs(ttl);

        let mut conn = self.conn.clone();
        let reply: Option<String> = redis::cmd("SET")
            .arg(&redis_key)
            .arg(&owner_token)
            .arg("NX")
            .arg("EX")
            .arg(ttl_secs)
            .query_async(&mut conn)
            .await
            .map_err(|err| CoreError::Internal(format!("redis lock acquire failed: {err}")))?;

        if reply.as_deref() == Some("OK") {
            let mut tokens = self.lock_tokens.lock().await;
            tokens.insert(redis_key, owner_token);
            return Ok(true);
        }

        Ok(false)
    }

    async fn release_lock(&self, key: &str) -> Result<(), CoreError> {
        let redis_key = self.lock_key(key);
        let maybe_token = {
            let mut tokens = self.lock_tokens.lock().await;
            tokens.remove(&redis_key)
        };

        let Some(owner_token) = maybe_token else {
            return Ok(());
        };

        let mut conn = self.conn.clone();
        let _: i32 = redis::cmd("EVAL")
            .arg(RELEASE_LOCK_LUA)
            .arg(1)
            .arg(&redis_key)
            .arg(&owner_token)
            .query_async(&mut conn)
            .await
            .map_err(|err| CoreError::Internal(format!("redis lock release failed: {err}")))?;

        Ok(())
    }

    async fn is_fresh(&self, key: &str) -> Result<bool, CoreError> {
        let redis_key = self.fresh_key(key);
        let mut conn = self.conn.clone();
        let exists: bool = redis::cmd("EXISTS")
            .arg(&redis_key)
            .query_async(&mut conn)
            .await
            .map_err(|err| CoreError::Internal(format!("redis EXISTS failed: {err}")))?;
        Ok(exists)
    }

    async fn mark_fresh(&self, key: &str, ttl: Duration) -> Result<(), CoreError> {
        let redis_key = self.fresh_key(key);
        let ttl_secs = Self::ttl_secs(ttl);
        let mut conn = self.conn.clone();
        let _: () = redis::cmd("SET")
            .arg(&redis_key)
            .arg("1")
            .arg("EX")
            .arg(ttl_secs)
            .query_async(&mut conn)
            .await
            .map_err(|err| CoreError::Internal(format!("redis mark_fresh failed: {err}")))?;
        Ok(())
    }

    async fn get_cached_value(&self, key: &str) -> Result<Option<String>, CoreError> {
        let redis_key = self.value_key(key);
        let mut conn = self.conn.clone();
        let val: Option<String> = redis::cmd("GET")
            .arg(&redis_key)
            .query_async(&mut conn)
            .await
            .map_err(|err| CoreError::Internal(format!("redis GET failed: {err}")))?;
        Ok(val)
    }

    async fn set_cached_value(
        &self,
        key: &str,
        value: &str,
        ttl: Duration,
    ) -> Result<(), CoreError> {
        let redis_key = self.value_key(key);
        let ttl_secs = Self::ttl_secs(ttl);
        let mut conn = self.conn.clone();
        let _: () = redis::cmd("SET")
            .arg(&redis_key)
            .arg(value)
            .arg("EX")
            .arg(ttl_secs)
            .query_async(&mut conn)
            .await
            .map_err(|err| CoreError::Internal(format!("redis SET failed: {err}")))?;
        Ok(())
    }
}

/// Builds a `RedisCacheCoordinator` when `config.resources.redis_url` is configured,
/// or falls back automatically to `InMemoryCacheCoordinator` when unset/empty.
pub async fn build_cache_coordinator(
    config: &AppConfig,
) -> Result<Arc<dyn CacheCoordinator>, CoreError> {
    if let Some(redis_url) = config
        .resources
        .redis_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        let coordinator =
            RedisCacheCoordinator::connect(redis_url, config.resources.redis_key_prefix.clone())
                .await?;
        return Ok(Arc::new(coordinator));
    }

    Ok(Arc::new(InMemoryCacheCoordinator::new()))
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

    #[tokio::test]
    async fn in_memory_coordinator_stores_and_expires_cached_values() {
        let cache = InMemoryCacheCoordinator::new();
        assert_eq!(cache.get_cached_value("fp:1").await.unwrap(), None);

        cache
            .set_cached_value("fp:1", "payload", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(
            cache.get_cached_value("fp:1").await.unwrap().as_deref(),
            Some("payload")
        );

        cache
            .set_cached_value("fp:expired", "old", Duration::from_millis(1))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(15)).await;
        assert_eq!(cache.get_cached_value("fp:expired").await.unwrap(), None);
    }

    #[tokio::test]
    async fn build_cache_coordinator_falls_back_to_in_memory_when_redis_url_unset_or_empty() {
        let mut config = AppConfig::default();
        assert!(config.resources.redis_url.is_none());
        let cache_none = build_cache_coordinator(&config).await.unwrap();
        assert!(cache_none
            .try_acquire_lock("fallback:1", Duration::from_secs(5))
            .await
            .unwrap());

        config.resources.redis_url = Some("   ".to_owned());
        let cache_empty = build_cache_coordinator(&config).await.unwrap();
        assert!(cache_empty
            .try_acquire_lock("fallback:2", Duration::from_secs(5))
            .await
            .unwrap());
    }
}
