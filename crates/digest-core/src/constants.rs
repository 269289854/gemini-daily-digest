//! Centralized protocol constants, HTTP headers, routes, and safe defaults.
//! Business and infrastructure modules MUST reference these constants rather
//! than repeating string or numeric literals inline.

/// Logical service identifier used in health responses and logs.
pub const SERVICE_NAME: &str = "gemini-daily-digest";

/// Standard health status literal.
pub const HEALTH_STATUS_OK: &str = "ok";

/// Crypto envelope version prefix for AES-256-GCM ciphertexts.
pub const CRYPTO_ENVELOPE_PREFIX: &str = "v1:";

/// Default server TCP bind address.
pub const DEFAULT_SERVER_BIND_ADDR: &str = "127.0.0.1:8080";

/// Default public web base URL used in development.
pub const DEFAULT_PUBLIC_BASE_URL: &str = "http://localhost:5173";

/// Default user schedule IANA timezone.
pub const DEFAULT_TIMEZONE: &str = "Asia/Shanghai";

/// Default daily digest trigger time (`HH:MM`).
pub const DEFAULT_DAILY_TIME: &str = "08:30";

/// Default AI provider display label.
pub const DEFAULT_AI_PROVIDER_LABEL: &str = "DeepSeek";

/// Default OpenAI-compatible AI base URL.
pub const DEFAULT_AI_BASE_URL: &str = "https://api.deepseek.com";

/// Default LLM model identifier.
pub const DEFAULT_AI_MODEL: &str = "deepseek-chat";

/// Default LLM sampling temperature.
pub const DEFAULT_LLM_TEMPERATURE: f32 = 0.2;

/// Default maximum completion tokens for daily summarization.
pub const DEFAULT_LLM_MAX_TOKENS: u32 = 4096;

/// Default outbound HTTP timeout for collectors (seconds).
pub const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 30;

/// Default LLM request timeout (seconds).
pub const DEFAULT_LLM_TIMEOUT_SECS: u64 = 90;

/// Default collector concurrency per run.
pub const DEFAULT_COLLECTOR_CONCURRENCY: usize = 6;

/// Default maximum retry attempts for transient upstream errors.
pub const DEFAULT_MAX_RETRY_ATTEMPTS: usize = 3;

/// Default initial exponential backoff delay (milliseconds).
pub const DEFAULT_RETRY_BACKOFF_MS: u64 = 500;

/// Default alert scan interval (minutes).
pub const DEFAULT_ALERT_INTERVAL_MINUTES: u32 = 60;

/// Default maximum items included in a single daily digest prompt.
pub const DEFAULT_MAX_ITEMS_PER_DIGEST: usize = 100;

/// Default pagination page size.
pub const DEFAULT_PAGE_SIZE: usize = 20;

/// Maximum pagination page size.
pub const DEFAULT_MAX_PAGE_SIZE: usize = 100;

/// Default local MCP HTTP bind host (loopback only).
pub const DEFAULT_MCP_BIND_HOST: &str = "127.0.0.1";

/// Default Tokio NIO worker threads sized for lean VPS deployments.
pub const DEFAULT_TOKIO_WORKER_THREADS: usize = 2;

/// Default PostgreSQL connection pool cap.
pub const DEFAULT_PG_MAX_CONNECTIONS: u32 = 5;

/// Default streaming response body cap per feed (2 MiB).
pub const DEFAULT_MAX_FEED_BYTES: usize = 2 * 1024 * 1024;

/// Default global concurrent upstream HTTP fetch cap.
pub const DEFAULT_GLOBAL_FETCH_CONCURRENCY: usize = 4;

/// Default canonical source freshness window (15 minutes).
pub const DEFAULT_SOURCE_FRESHNESS_TTL_SECS: u64 = 900;

/// Default retention days for full unbookmarked item bodies.
pub const DEFAULT_ITEM_BODY_RETENTION_DAYS: u32 = 180;

/// Default Redis namespace prefix.
pub const DEFAULT_REDIS_KEY_PREFIX: &str = "gdd:v1:";

/// HTTP header names (lowercase canonical form).
pub const HEADER_AUTHORIZATION: &str = "authorization";
pub const HEADER_BEARER_PREFIX: &str = "Bearer ";
pub const HEADER_REQUEST_ID: &str = "x-request-id";
pub const HEADER_MCP_TOKEN: &str = "x-digest-mcp-token";
pub const HEADER_IF_NONE_MATCH: &str = "if-none-match";
pub const HEADER_IF_MODIFIED_SINCE: &str = "if-modified-since";
pub const HEADER_ETAG: &str = "etag";
pub const HEADER_LAST_MODIFIED: &str = "last-modified";

/// Core API routes.
pub const ROUTE_HEALTH: &str = "/health";
pub const API_PREFIX: &str = "/api/v1";

/// Environment variable keys.
pub const ENV_SERVER_BIND_ADDR: &str = "SERVER_BIND_ADDR";
pub const ENV_PUBLIC_BASE_URL: &str = "PUBLIC_BASE_URL";
pub const ENV_DATABASE_URL: &str = "DATABASE_URL";
pub const ENV_REDIS_URL: &str = "REDIS_URL";
pub const ENV_AUTH_MODE: &str = "AUTH_MODE";
pub const ENV_OIDC_ISSUER_URL: &str = "OIDC_ISSUER_URL";
pub const ENV_OIDC_CLIENT_ID: &str = "OIDC_CLIENT_ID";
pub const ENV_OIDC_AUDIENCE: &str = "OIDC_AUDIENCE";
pub const ENV_APP_MASTER_KEY_B64: &str = "APP_MASTER_KEY_B64";
pub const ENV_SSRF_ALLOWLIST_HOSTS: &str = "SSRF_ALLOWLIST_HOSTS";
pub const ENV_OUTBOUND_PROXY_URL: &str = "OUTBOUND_PROXY_URL";
pub const ENV_TOKIO_WORKER_THREADS: &str = "TOKIO_WORKER_THREADS";
pub const ENV_PG_MAX_CONNECTIONS: &str = "PG_MAX_CONNECTIONS";
pub const ENV_MAX_FEED_BYTES: &str = "MAX_FEED_BYTES";
pub const ENV_GLOBAL_FETCH_CONCURRENCY: &str = "GLOBAL_FETCH_CONCURRENCY";
pub const ENV_SOURCE_FRESHNESS_TTL_SECS: &str = "SOURCE_FRESHNESS_TTL_SECS";
pub const ENV_ITEM_BODY_RETENTION_DAYS: &str = "ITEM_BODY_RETENTION_DAYS";
