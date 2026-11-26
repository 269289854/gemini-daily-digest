//! Server-specific constants, route paths, auth defaults, SSE tuning,
//! and scheduler intervals. Shared protocol constants remain in `digest_core::constants`.

/// Sub-routes mounted under `digest_core::constants::API_PREFIX` (`/api/v1`).
pub const SUBROUTE_AUTH_CONFIG: &str = "/auth/config";
pub const SUBROUTE_ME: &str = "/me";
pub const SUBROUTE_DIGESTS: &str = "/digests";
pub const SUBROUTE_DIGEST_BY_DATE: &str = "/digests/{date}";
pub const SUBROUTE_DIGEST_MARKDOWN: &str = "/digests/{date}/markdown";
pub const SUBROUTE_SOURCE_DIGESTS: &str = "/sources/{id}/digests";
pub const SUBROUTE_SOURCE_ITEMS: &str = "/sources/{id}/items";
pub const SUBROUTE_ITEMS: &str = "/items";
pub const SUBROUTE_BOOKMARKS: &str = "/bookmarks";
pub const SUBROUTE_BOOKMARK_BY_ITEM: &str = "/bookmarks/{item_id}";
pub const SUBROUTE_SETTINGS_AI: &str = "/settings/ai";
pub const SUBROUTE_SETTINGS_AI_TEST: &str = "/settings/ai/test";
pub const SUBROUTE_SETTINGS_SOURCES: &str = "/settings/sources";
pub const SUBROUTE_SETTINGS_SOURCE_BY_ID: &str = "/settings/sources/{id}";
pub const SUBROUTE_SETTINGS_SOURCES_IMPORT_PRESET: &str = "/settings/sources/import-preset";
pub const SUBROUTE_SETTINGS_SOURCES_EXPORT: &str = "/settings/sources/export";
pub const SUBROUTE_SETTINGS_SOURCES_IMPORT: &str = "/settings/sources/import";
pub const SUBROUTE_SETTINGS_SOURCE_TEST: &str = "/settings/sources/{id}/test";
pub const SUBROUTE_SETTINGS_CHANNELS: &str = "/settings/channels";
pub const SUBROUTE_SETTINGS_CHANNEL_BY_ID: &str = "/settings/channels/{id}";
pub const SUBROUTE_SETTINGS_CHANNEL_TEST: &str = "/settings/channels/{id}/test";
pub const SUBROUTE_SETTINGS_SCHEDULE: &str = "/settings/schedule";
pub const SUBROUTE_SETTINGS_WATCHLIST: &str = "/settings/watchlist";
pub const SUBROUTE_SETTINGS_WATCHLIST_BY_ID: &str = "/settings/watchlist/{id}";
pub const SUBROUTE_RUNS_TRIGGER: &str = "/runs/trigger";
pub const SUBROUTE_RUNS: &str = "/runs";
pub const SUBROUTE_RUNS_STATS: &str = "/runs/stats";
pub const SUBROUTE_DELIVERY_RESEND: &str = "/deliveries/{id}/resend";
pub const SUBROUTE_EVENTS_STREAM: &str = "/events/stream";

/// Default OIDC scopes advertised by `/api/v1/auth/config`.
pub const DEFAULT_OIDC_SCOPES: &[&str] = &["openid", "profile", "email"];

/// Standard OIDC discovery document path suffix.
pub const OIDC_WELL_KNOWN_SUFFIX: &str = "/.well-known/openid-configuration";

/// Default role assigned to newly provisioned users.
pub const DEFAULT_USER_ROLE: &str = "member";

/// Deterministic local development user identity for loopback-only `dev_bypass`.
pub const DEV_BYPASS_OIDC_SUB: &str = "dev-bypass-local";
pub const DEV_BYPASS_EMAIL: &str = "dev@example.com";
pub const DEV_BYPASS_DISPLAY_NAME: &str = "Local Developer";

/// Domain-separation seed used ONLY when `AUTH_MODE=dev_bypass` and `APP_MASTER_KEY_B64` is unset.
pub const DEV_BYPASS_FALLBACK_CIPHER_LABEL: &str = "gemini-daily-digest:dev-bypass-local-key:v1";

/// JWT validation leeway (seconds), JWKS cache TTL (seconds), and allowed signing algorithm.
pub const JWT_LEEWAY_SECS: u64 = 60;
pub const JWKS_CACHE_TTL_SECS: u64 = 300;
pub const OIDC_HTTP_TIMEOUT_SECS: u64 = 10;
pub const OIDC_JWT_ALGORITHM: jsonwebtoken::Algorithm = jsonwebtoken::Algorithm::RS256;

/// SSE heartbeat interval (seconds) and per-user broadcast channel capacity.
pub const SSE_HEARTBEAT_INTERVAL_SECS: u64 = 15;
pub const SSE_CHANNEL_CAPACITY: usize = 64;

/// Background scheduler tick intervals and batch caps.
pub const SCHEDULER_POLL_TICK_SECS: u64 = 60;
pub const SCHEDULER_COMPOSE_TICK_SECS: u64 = 60;
pub const SCHEDULER_ALERT_TICK_SECS: u64 = 60;
pub const SCHEDULER_RETENTION_TICK_SECS: u64 = 3600;
pub const SCHEDULER_POLL_BATCH_LIMIT: usize = 50;

/// Source export JSON schema version.
pub const SOURCE_EXPORT_FORMAT_VERSION: u32 = 1;

/// HTTP content-type header value for raw Markdown digest downloads.
pub const CONTENT_TYPE_MARKDOWN_UTF8: &str = "text/markdown; charset=utf-8";

/// Encrypted source config JSON object keys (`canonical_sources.config_json_enc`).
pub const SOURCE_CONFIG_KEY_HANDLE_OR_URL: &str = "handleOrUrl";
pub const SOURCE_CONFIG_KEY_CREDENTIALS: &str = "credentials";

/// Maximum sample item titles returned by single-source live preview test.
pub const SOURCE_TEST_SAMPLE_TITLE_LIMIT: usize = 5;

/// Default prompt snippet used by AI endpoint connectivity checks.
pub const DEFAULT_AI_CONNECTIVITY_TEST_PROMPT: &str = "Reply with OK";
