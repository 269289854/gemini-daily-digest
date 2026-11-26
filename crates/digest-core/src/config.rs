//! Typed runtime configuration replacing inline tuning magic numbers.

use std::num::NonZeroUsize;
use std::time::Duration;

use crate::constants::{
    DEFAULT_AI_BASE_URL, DEFAULT_AI_MODEL, DEFAULT_AI_PROVIDER_LABEL,
    DEFAULT_ALERT_INTERVAL_MINUTES, DEFAULT_COLLECTOR_BACKOFF_STEPS_SECS,
    DEFAULT_COLLECTOR_CONCURRENCY, DEFAULT_DAILY_TIME, DEFAULT_DIGEST_FINGERPRINT_TTL_SECS,
    DEFAULT_GLOBAL_FETCH_CONCURRENCY, DEFAULT_HTTP_MAX_REDIRECTS,
    DEFAULT_HTTP_POOL_IDLE_TIMEOUT_SECS, DEFAULT_HTTP_TIMEOUT_SECS,
    DEFAULT_ITEM_BODY_RETENTION_DAYS, DEFAULT_LLM_MAX_TOKENS, DEFAULT_LLM_SYSTEM_PROMPT,
    DEFAULT_LLM_TEMPERATURE, DEFAULT_LLM_TIMEOUT_SECS, DEFAULT_MAX_FEED_BYTES,
    DEFAULT_MAX_ITEMS_PER_DIGEST, DEFAULT_MAX_PAGE_SIZE, DEFAULT_MAX_RETRY_ATTEMPTS,
    DEFAULT_PAGE_SIZE, DEFAULT_PG_MAX_CONNECTIONS, DEFAULT_PUBLIC_BASE_URL,
    DEFAULT_REDIS_KEY_PREFIX, DEFAULT_RETRY_BACKOFF_MS, DEFAULT_SERVER_BIND_ADDR,
    DEFAULT_SINGLEFLIGHT_LOCK_TTL_SECS, DEFAULT_SMTP_TIMEOUT_SECS,
    DEFAULT_SOURCE_FRESHNESS_TTL_SECS, DEFAULT_TELEGRAM_API_BASE_URL, DEFAULT_TIMEZONE,
    DEFAULT_TOKIO_WORKER_THREADS, DEFAULT_USER_AGENT, DEFAULT_WEBHOOK_TIMEOUT_SECS, ENV_AUTH_MODE,
    ENV_GLOBAL_FETCH_CONCURRENCY, ENV_ITEM_BODY_RETENTION_DAYS, ENV_MAX_FEED_BYTES,
    ENV_OUTBOUND_PROXY_URL, ENV_PG_MAX_CONNECTIONS, ENV_PUBLIC_BASE_URL, ENV_REDIS_URL,
    ENV_SERVER_BIND_ADDR, ENV_SOURCE_FRESHNESS_TTL_SECS, ENV_SSRF_ALLOWLIST_HOSTS,
    ENV_TOKIO_WORKER_THREADS,
};
use crate::domain::AuthMode;
use crate::error::CoreError;

/// Collector resilience and concurrency tuning parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectorTuning {
    pub request_timeout: Duration,
    pub max_concurrency: NonZeroUsize,
    pub max_retry_attempts: NonZeroUsize,
    pub initial_backoff: Duration,
    pub failure_backoff_steps: Vec<Duration>,
    pub user_agent: String,
    pub max_redirects: usize,
    pub pool_idle_timeout: Duration,
}

impl Default for CollectorTuning {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS),
            max_concurrency: NonZeroUsize::new(DEFAULT_COLLECTOR_CONCURRENCY)
                .unwrap_or(NonZeroUsize::MIN),
            max_retry_attempts: NonZeroUsize::new(DEFAULT_MAX_RETRY_ATTEMPTS)
                .unwrap_or(NonZeroUsize::MIN),
            initial_backoff: Duration::from_millis(DEFAULT_RETRY_BACKOFF_MS),
            failure_backoff_steps: DEFAULT_COLLECTOR_BACKOFF_STEPS_SECS
                .iter()
                .copied()
                .map(Duration::from_secs)
                .collect(),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            max_redirects: DEFAULT_HTTP_MAX_REDIRECTS,
            pool_idle_timeout: Duration::from_secs(DEFAULT_HTTP_POOL_IDLE_TIMEOUT_SECS),
        }
    }
}

/// LLM summarization and fallback tuning parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct LlmTuning {
    pub default_provider_label: String,
    pub default_base_url: String,
    pub default_model: String,
    pub default_system_prompt: String,
    pub request_timeout: Duration,
    pub default_temperature: f32,
    pub default_max_tokens: u32,
    pub max_items_per_digest: usize,
}

impl Default for LlmTuning {
    fn default() -> Self {
        Self {
            default_provider_label: DEFAULT_AI_PROVIDER_LABEL.to_owned(),
            default_base_url: DEFAULT_AI_BASE_URL.to_owned(),
            default_model: DEFAULT_AI_MODEL.to_owned(),
            default_system_prompt: DEFAULT_LLM_SYSTEM_PROMPT.to_owned(),
            request_timeout: Duration::from_secs(DEFAULT_LLM_TIMEOUT_SECS),
            default_temperature: DEFAULT_LLM_TEMPERATURE,
            default_max_tokens: DEFAULT_LLM_MAX_TOKENS,
            max_items_per_digest: DEFAULT_MAX_ITEMS_PER_DIGEST,
        }
    }
}

/// Scheduler and pagination tuning defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerTuning {
    pub default_timezone: String,
    pub default_daily_time: String,
    pub default_alert_interval: Duration,
    pub default_page_size: usize,
    pub max_page_size: usize,
}

impl Default for SchedulerTuning {
    fn default() -> Self {
        Self {
            default_timezone: DEFAULT_TIMEZONE.to_owned(),
            default_daily_time: DEFAULT_DAILY_TIME.to_owned(),
            default_alert_interval: Duration::from_secs(
                u64::from(DEFAULT_ALERT_INTERVAL_MINUTES) * 60,
            ),
            default_page_size: DEFAULT_PAGE_SIZE,
            max_page_size: DEFAULT_MAX_PAGE_SIZE,
        }
    }
}

/// Outbound notifier timeouts and endpoint defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifierTuning {
    pub smtp_timeout: Duration,
    pub webhook_timeout: Duration,
    pub telegram_api_base_url: String,
}

impl Default for NotifierTuning {
    fn default() -> Self {
        Self {
            smtp_timeout: Duration::from_secs(DEFAULT_SMTP_TIMEOUT_SECS),
            webhook_timeout: Duration::from_secs(DEFAULT_WEBHOOK_TIMEOUT_SECS),
            telegram_api_base_url: DEFAULT_TELEGRAM_API_BASE_URL.to_owned(),
        }
    }
}

/// NIO runtime, bandwidth, memory, and cache coordinator hard limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLimitsTuning {
    pub tokio_worker_threads: NonZeroUsize,
    pub pg_max_connections: u32,
    pub max_feed_bytes: NonZeroUsize,
    pub global_fetch_concurrency: NonZeroUsize,
    pub source_freshness_ttl: Duration,
    pub singleflight_lock_ttl: Duration,
    pub digest_fingerprint_ttl: Duration,
    pub item_body_retention_days: u32,
    pub redis_key_prefix: String,
    pub redis_url: Option<String>,
}

impl Default for ResourceLimitsTuning {
    fn default() -> Self {
        Self {
            tokio_worker_threads: NonZeroUsize::new(DEFAULT_TOKIO_WORKER_THREADS)
                .unwrap_or(NonZeroUsize::MIN),
            pg_max_connections: DEFAULT_PG_MAX_CONNECTIONS,
            max_feed_bytes: NonZeroUsize::new(DEFAULT_MAX_FEED_BYTES).unwrap_or(NonZeroUsize::MIN),
            global_fetch_concurrency: NonZeroUsize::new(DEFAULT_GLOBAL_FETCH_CONCURRENCY)
                .unwrap_or(NonZeroUsize::MIN),
            source_freshness_ttl: Duration::from_secs(DEFAULT_SOURCE_FRESHNESS_TTL_SECS),
            singleflight_lock_ttl: Duration::from_secs(DEFAULT_SINGLEFLIGHT_LOCK_TTL_SECS),
            digest_fingerprint_ttl: Duration::from_secs(DEFAULT_DIGEST_FINGERPRINT_TTL_SECS),
            item_body_retention_days: DEFAULT_ITEM_BODY_RETENTION_DAYS,
            redis_key_prefix: DEFAULT_REDIS_KEY_PREFIX.to_owned(),
            redis_url: None,
        }
    }
}

/// Top-level application configuration hydrated from environment variables.
#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    pub server_bind_addr: String,
    pub public_base_url: String,
    pub auth_mode: AuthMode,
    pub ssrf_allowlist_hosts: Vec<String>,
    pub outbound_proxy_url: Option<String>,
    pub collector: CollectorTuning,
    pub llm: LlmTuning,
    pub scheduler: SchedulerTuning,
    pub notifier: NotifierTuning,
    pub resources: ResourceLimitsTuning,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_bind_addr: DEFAULT_SERVER_BIND_ADDR.to_owned(),
            public_base_url: DEFAULT_PUBLIC_BASE_URL.to_owned(),
            auth_mode: AuthMode::DevBypass,
            ssrf_allowlist_hosts: Vec::new(),
            outbound_proxy_url: None,
            collector: CollectorTuning::default(),
            llm: LlmTuning::default(),
            scheduler: SchedulerTuning::default(),
            notifier: NotifierTuning::default(),
            resources: ResourceLimitsTuning::default(),
        }
    }
}

fn parse_env_nonzero_usize(key: &str) -> Result<Option<NonZeroUsize>, CoreError> {
    match std::env::var(key) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let parsed: usize = trimmed.parse().map_err(|_| {
                CoreError::InvalidConfig(format!("{key} must be a positive integer"))
            })?;
            let nz = NonZeroUsize::new(parsed)
                .ok_or_else(|| CoreError::InvalidConfig(format!("{key} must be >= 1")))?;
            Ok(Some(nz))
        }
        Err(_) => Ok(None),
    }
}

fn parse_env_positive_u32(key: &str) -> Result<Option<u32>, CoreError> {
    match std::env::var(key) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let parsed: u32 = trimmed.parse().map_err(|_| {
                CoreError::InvalidConfig(format!("{key} must be a positive integer"))
            })?;
            if parsed == 0 {
                return Err(CoreError::InvalidConfig(format!("{key} must be >= 1")));
            }
            Ok(Some(parsed))
        }
        Err(_) => Ok(None),
    }
}

fn parse_env_u64(key: &str) -> Result<Option<u64>, CoreError> {
    match std::env::var(key) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let parsed: u64 = trimmed.parse().map_err(|_| {
                CoreError::InvalidConfig(format!("{key} must be a non-negative integer"))
            })?;
            Ok(Some(parsed))
        }
        Err(_) => Ok(None),
    }
}

impl AppConfig {
    /// Builds `AppConfig` using environment overrides when present.
    pub fn from_env() -> Result<Self, CoreError> {
        let mut cfg = Self::default();

        if let Ok(bind_addr) = std::env::var(ENV_SERVER_BIND_ADDR) {
            if !bind_addr.trim().is_empty() {
                cfg.server_bind_addr = bind_addr;
            }
        }
        if let Ok(public_url) = std::env::var(ENV_PUBLIC_BASE_URL) {
            if !public_url.trim().is_empty() {
                cfg.public_base_url = public_url;
            }
        }
        if let Ok(auth_mode_raw) = std::env::var(ENV_AUTH_MODE) {
            if !auth_mode_raw.trim().is_empty() {
                cfg.auth_mode = auth_mode_raw.parse()?;
            }
        }
        if let Ok(allowlist_raw) = std::env::var(ENV_SSRF_ALLOWLIST_HOSTS) {
            cfg.ssrf_allowlist_hosts = allowlist_raw
                .split(',')
                .map(str::trim)
                .filter(|host| !host.is_empty())
                .map(str::to_ascii_lowercase)
                .collect();
        }
        if let Ok(proxy_raw) = std::env::var(ENV_OUTBOUND_PROXY_URL) {
            let trimmed = proxy_raw.trim();
            if !trimmed.is_empty() {
                cfg.outbound_proxy_url = Some(trimmed.to_owned());
            }
        }
        if let Ok(redis_raw) = std::env::var(ENV_REDIS_URL) {
            let trimmed = redis_raw.trim();
            if !trimmed.is_empty() {
                cfg.resources.redis_url = Some(trimmed.to_owned());
            }
        }
        if let Some(threads) = parse_env_nonzero_usize(ENV_TOKIO_WORKER_THREADS)? {
            cfg.resources.tokio_worker_threads = threads;
        }
        if let Some(pg_conns) = parse_env_positive_u32(ENV_PG_MAX_CONNECTIONS)? {
            cfg.resources.pg_max_connections = pg_conns;
        }
        if let Some(max_bytes) = parse_env_nonzero_usize(ENV_MAX_FEED_BYTES)? {
            cfg.resources.max_feed_bytes = max_bytes;
        }
        if let Some(conc) = parse_env_nonzero_usize(ENV_GLOBAL_FETCH_CONCURRENCY)? {
            cfg.resources.global_fetch_concurrency = conc;
        }
        if let Some(ttl_secs) = parse_env_u64(ENV_SOURCE_FRESHNESS_TTL_SECS)? {
            cfg.resources.source_freshness_ttl = Duration::from_secs(ttl_secs);
        }
        if let Some(days) = parse_env_positive_u32(ENV_ITEM_BODY_RETENTION_DAYS)? {
            cfg.resources.item_body_retention_days = days;
        }

        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values_are_consistent_and_nonzero() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.server_bind_addr, DEFAULT_SERVER_BIND_ADDR);
        assert_eq!(cfg.auth_mode, AuthMode::DevBypass);
        assert!(cfg.collector.max_concurrency.get() >= 1);
        assert!(cfg.collector.max_retry_attempts.get() >= 1);
        assert!(cfg.scheduler.default_page_size <= cfg.scheduler.max_page_size);
        assert_eq!(cfg.llm.default_model, DEFAULT_AI_MODEL);
        assert_eq!(
            cfg.resources.tokio_worker_threads.get(),
            DEFAULT_TOKIO_WORKER_THREADS
        );
        assert_eq!(cfg.resources.pg_max_connections, DEFAULT_PG_MAX_CONNECTIONS);
        assert_eq!(cfg.resources.max_feed_bytes.get(), DEFAULT_MAX_FEED_BYTES);
        assert_eq!(
            cfg.resources.global_fetch_concurrency.get(),
            DEFAULT_GLOBAL_FETCH_CONCURRENCY
        );
        assert_eq!(cfg.resources.redis_key_prefix, DEFAULT_REDIS_KEY_PREFIX);
        assert!(cfg.resources.redis_url.is_none());
    }
}
