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
pub const HEADER_LOCATION: &str = "location";

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
pub const ENV_GITHUB_TOKEN: &str = "GITHUB_TOKEN";

/// Additional HTTP header names and protocol literals.
pub const HEADER_ACCEPT: &str = "accept";
pub const HEADER_CONTENT_TYPE: &str = "content-type";
pub const HEADER_USER_AGENT: &str = "user-agent";
pub const HEADER_X_GITHUB_API_VERSION: &str = "x-github-api-version";
pub const HEADER_WEBHOOK_SIGNATURE: &str = "x-digest-signature-256";
pub const CONTENT_TYPE_JSON: &str = "application/json";
pub const GITHUB_API_VERSION: &str = "2022-11-28";

/// Envelope encryption and secret masking constants.
pub const CRYPTO_KEY_BYTE_LEN: usize = 32;
pub const CRYPTO_NONCE_BYTE_LEN: usize = 12;
pub const SECRET_MASK_VISIBLE_PREFIX_LEN: usize = 4;
pub const SECRET_MASK_VISIBLE_SUFFIX_LEN: usize = 4;
pub const SECRET_MASK_MIN_PARTIAL_LEN: usize = 10;
pub const SECRET_MASK_REDACTED_PLACEHOLDER: &str = "****";
pub const SECRET_MASK_CONFIGURED_PLACEHOLDER: &str = "configured";

/// DNS-aware SSRF guard rules.
pub const SSRF_ALLOWED_SCHEMES: &[&str] = &["http", "https"];
pub const SSRF_BLOCKED_METADATA_HOSTS: &[&str] = &[
    "localhost",
    "localhost.localdomain",
    "metadata.google.internal",
    "metadata.goog",
    "169.254.169.254",
    "100.100.100.200",
    "fd00:ec2::254",
];
pub const SSRF_BLOCKED_IPV4_CIDRS: &[&str] = &[
    "0.0.0.0/8",
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.0.0.0/24",
    "192.0.2.0/24",
    "192.168.0.0/16",
    "198.18.0.0/15",
    "198.51.100.0/24",
    "203.0.113.0/24",
    "224.0.0.0/4",
    "240.0.0.0/4",
    "255.255.255.255/32",
];
pub const SSRF_BLOCKED_IPV6_CIDRS: &[&str] = &[
    "::/128",
    "::1/128",
    "::ffff:0:0/96",
    "64:ff9b::/96",
    "100::/64",
    "2001::/32",
    "2001:db8::/32",
    "fc00::/7",
    "fe80::/10",
    "ff00::/8",
];

/// Gemini Ecosystem Preset Pack identifiers and canonical categories.
pub const PRESET_PACK_ID_GEMINI_ECOSYSTEM: &str = "gemini-ecosystem-v1";
pub const PRESET_PACK_NAME_GEMINI_ECOSYSTEM: &str = "Gemini Ecosystem Preset Pack";
pub const PRESET_CATEGORY_OFFICIAL: &str = "Official";
pub const PRESET_CATEGORY_RELEASES: &str = "Releases";
pub const PRESET_CATEGORY_COMMUNITY: &str = "Community";
pub const PRESET_CATEGORY_NEWS: &str = "News";
pub const PRESET_CATEGORY_SOCIAL: &str = "Social";

/// Collector upstream base URLs, HTTP tuning, and backoff schedule.
pub const DEFAULT_USER_AGENT: &str =
    "gemini-daily-digest/0.1 (+https://github.com/gemini-daily-digest)";
pub const DEFAULT_GITHUB_API_BASE_URL: &str = "https://api.github.com";
pub const DEFAULT_REDDIT_BASE_URL: &str = "https://www.reddit.com";
pub const DEFAULT_HACKER_NEWS_ALGOLIA_BASE_URL: &str = "https://hn.algolia.com/api/v1";
pub const DEFAULT_GOOGLE_NEWS_RSS_BASE_URL: &str = "https://news.google.com/rss/search";
pub const DEFAULT_X_API_BASE_URL: &str = "https://api.x.com/2";
pub const DEFAULT_HTTP_MAX_REDIRECTS: usize = 5;
pub const DEFAULT_HTTP_POOL_IDLE_TIMEOUT_SECS: u64 = 90;
/// Adaptive failure backoff durations in seconds (`1h -> 4h -> 12h -> 24h`).
pub const DEFAULT_COLLECTOR_BACKOFF_STEPS_SECS: &[u64] = &[3600, 14400, 43200, 86400];

/// URL normalization tracking query parameters stripped during canonicalization.
pub const TRACKING_QUERY_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
    "fbclid",
    "gclid",
    "mc_cid",
    "mc_eid",
    "ref",
    "ref_src",
    "spm",
    "igshid",
];

/// Pipeline importance scoring weights.
pub const DEFAULT_BASE_IMPORTANCE_SCORE: i32 = 10;
pub const DEFAULT_KEYWORD_MATCH_BONUS: i32 = 25;
pub const DEFAULT_URGENT_PRIORITY_BONUS: i32 = 50;
pub const DEFAULT_HIGH_PRIORITY_BONUS: i32 = 25;
pub const DEFAULT_RECENCY_24H_BONUS: i32 = 15;
/// Bonus awarded when an item includes a non-empty summary body.
pub const DEFAULT_SUMMARY_RICHNESS_BONUS: i32 = 5;

/// OpenAI-compatible LLM endpoint path, default prompt, and fallback label.
pub const LLM_CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";
pub const LLM_CHAT_COMPLETIONS_SUFFIX: &str = "/chat/completions";
pub const DEFAULT_LLM_SYSTEM_PROMPT: &str = "You are a precise technical intelligence analyst summarizing Gemini and AI ecosystem updates. Produce concise, factual summaries grounded strictly in the provided items.";
pub const FALLBACK_MODEL_LABEL: &str = "deterministic-fallback";

/// Outbound notifier endpoint defaults, timeouts, and message size caps.
pub const DEFAULT_TELEGRAM_API_BASE_URL: &str = "https://api.telegram.org";
pub const DEFAULT_SMTP_PORT: u16 = 587;
pub const DEFAULT_SMTP_IMPLICIT_TLS_PORT: u16 = 465;
pub const DEFAULT_SMTP_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_WEBHOOK_TIMEOUT_SECS: u64 = 15;
pub const TELEGRAM_MAX_TEXT_CHARS: usize = 4000;
pub const FEISHU_MAX_MARKDOWN_CHARS: usize = 8000;
pub const DINGTALK_MAX_MARKDOWN_CHARS: usize = 8000;
pub const WECOM_MAX_MARKDOWN_CHARS: usize = 3800;

/// Cache coordinator key namespaces and TTL defaults.
pub const CACHE_LOCK_NAMESPACE: &str = "lock:";
pub const CACHE_FRESH_NAMESPACE: &str = "fresh:";
pub const CACHE_DIGEST_FINGERPRINT_NAMESPACE: &str = "digest_fp:";
pub const DEFAULT_SINGLEFLIGHT_LOCK_TTL_SECS: u64 = 120;
pub const DEFAULT_DIGEST_FINGERPRINT_TTL_SECS: u64 = 86400;

/// Maximum fixed-point passes for CommonMark AST-based sanitization.
pub const MARKDOWN_SANITIZE_MAX_PASSES: usize = 32;

/// Safe destination placeholder written when neutralizing dangerous reference definitions.
pub const MARKDOWN_SAFE_REF_PLACEHOLDER: &str = "#";
