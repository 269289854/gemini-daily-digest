/**
 * Centralized API paths, frontend routes, HTTP headers, client storage keys,
 * and shared safe defaults mirroring `digest-core` and `digest-server`.
 */

export const API_PREFIX = '/api/v1' as const;

export const API_ROUTES = {
  HEALTH: '/health',
  AUTH_CONFIG: `${API_PREFIX}/auth/config`,
  ME: `${API_PREFIX}/me`,
  DIGESTS: `${API_PREFIX}/digests`,
  digestByDate: (date: string) => `${API_PREFIX}/digests/${encodeURIComponent(date)}`,
  digestMarkdown: (date: string) => `${API_PREFIX}/digests/${encodeURIComponent(date)}/markdown`,
  sourceDigests: (sourceId: string) => `${API_PREFIX}/sources/${encodeURIComponent(sourceId)}/digests`,
  sourceItems: (sourceId: string) => `${API_PREFIX}/sources/${encodeURIComponent(sourceId)}/items`,
  ITEMS: `${API_PREFIX}/items`,
  BOOKMARKS: `${API_PREFIX}/bookmarks`,
  bookmarkByItem: (itemId: string) => `${API_PREFIX}/bookmarks/${encodeURIComponent(itemId)}`,
  SETTINGS_AI: `${API_PREFIX}/settings/ai`,
  SETTINGS_AI_TEST: `${API_PREFIX}/settings/ai/test`,
  SETTINGS_AI_MODELS: `${API_PREFIX}/settings/ai/models`,
  SETTINGS_SOURCES: `${API_PREFIX}/settings/sources`,
  sourceById: (sourceId: string) => `${API_PREFIX}/settings/sources/${encodeURIComponent(sourceId)}`,
  SETTINGS_SOURCES_IMPORT_PRESET: `${API_PREFIX}/settings/sources/import-preset`,
  SETTINGS_SOURCES_EXPORT: `${API_PREFIX}/settings/sources/export`,
  SETTINGS_SOURCES_IMPORT: `${API_PREFIX}/settings/sources/import`,
  sourceTest: (sourceId: string) => `${API_PREFIX}/settings/sources/${encodeURIComponent(sourceId)}/test`,
  SETTINGS_CHANNELS: `${API_PREFIX}/settings/channels`,
  channelById: (channelId: string) => `${API_PREFIX}/settings/channels/${encodeURIComponent(channelId)}`,
  channelTest: (channelId: string) => `${API_PREFIX}/settings/channels/${encodeURIComponent(channelId)}/test`,
  SETTINGS_SCHEDULE: `${API_PREFIX}/settings/schedule`,
  SETTINGS_WATCHLIST: `${API_PREFIX}/settings/watchlist`,
  watchlistById: (ruleId: string) => `${API_PREFIX}/settings/watchlist/${encodeURIComponent(ruleId)}`,
  RUNS_TRIGGER: `${API_PREFIX}/runs/trigger`,
  RUNS: `${API_PREFIX}/runs`,
  RUNS_STATS: `${API_PREFIX}/runs/stats`,
  resendDelivery: (deliveryId: string) => `${API_PREFIX}/deliveries/${encodeURIComponent(deliveryId)}/resend`,
  EVENTS_STREAM: `${API_PREFIX}/events/stream`,
  ADMIN_PLATFORM: `${API_PREFIX}/admin/platform`,
  ADMIN_PLATFORM_X_TEST: `${API_PREFIX}/admin/platform/x/test`,
} as const;

export const APP_ROUTES = {
  DAILY: '/daily',
  SOURCES: '/sources',
  HISTORY: '/history',
  RUNS: '/runs',
  SETTINGS: '/settings',
  AUTH_CALLBACK: '/auth/callback',
} as const;

export const STORAGE_KEYS = {
  SERVER_BASE_URL: 'gemini_digest.server_base_url',
  PKCE_VERIFIER: 'gemini_digest.pkce_verifier',
  OIDC_STATE: 'gemini_digest.oidc_state',
  OIDC_REDIRECT_URI: 'gemini_digest.oidc_redirect_uri',
  ACCESS_TOKEN: 'gemini_digest.access_token',
  ID_TOKEN: 'gemini_digest.id_token',
  TOKEN_EXPIRES_AT: 'gemini_digest.token_expires_at',
  POST_LOGIN_REDIRECT: 'gemini_digest.post_login_redirect',
  MCP_ACCESS_MODE: 'gemini_digest.mcp_access_mode',
  MCP_PORT: 'gemini_digest.mcp_port',
  ACTIVE_DIGEST_DATE: 'gemini_digest.active_digest_date',
  LOCALE: 'gemini_digest.locale',
} as const;

export const TAURI_COMMANDS = {
  PREPARE_OIDC_LOOPBACK: 'prepare_oidc_loopback',
  OPEN_OIDC_SYSTEM_LOGIN: 'open_oidc_system_login',
  OIDC_HTTP_REQUEST: 'oidc_http_request',
  OPEN_EXTERNAL_URL: 'open_external_url',
  SEND_NATIVE_NOTIFICATION: 'send_native_notification',
  SYNC_DESKTOP_RUNTIME_CONFIG: 'sync_desktop_runtime_config',
  GET_MCP_BRIDGE_STATUS: 'get_mcp_bridge_status',
  GET_MCP_LOCAL_TOKEN: 'get_mcp_local_token',
  ROTATE_MCP_TOKEN: 'rotate_mcp_token',
} as const;

export const TAURI_EVENTS = {
  DESKTOP_NAVIGATE: 'desktop://navigate',
} as const;

export const HTTP_HEADERS = {
  AUTHORIZATION: 'authorization',
  BEARER_PREFIX: 'Bearer ',
  REQUEST_ID: 'x-request-id',
  MCP_TOKEN: 'x-digest-mcp-token',
  CONTENT_TYPE: 'content-type',
  ACCEPT: 'accept',
  CONTENT_TYPE_JSON: 'application/json',
  CONTENT_TYPE_TEXT_EVENT_STREAM: 'text/event-stream',
} as const;

export const DEFAULTS = {
  TIMEZONE: 'Asia/Shanghai',
  DAILY_TIME: '08:30',
  ALERT_INTERVAL_MINUTES: 60,
  AI_PROVIDER_LABEL: 'DeepSeek',
  AI_BASE_URL: 'https://api.deepseek.com',
  AI_MODEL: 'deepseek-chat',
  LLM_TEMPERATURE: 0.2,
  LLM_MAX_TOKENS: 4096,
  LLM_SYSTEM_PROMPT:
    'You are a precise technical intelligence analyst summarizing Gemini and AI ecosystem updates. Produce concise, factual summaries grounded strictly in the provided items.',
  FALLBACK_MODEL_LABEL: 'deterministic-fallback',
  PAGE_SIZE: 20,
  MAX_PAGE_SIZE: 100,
  SMTP_PORT: 587,
  SMTP_IMPLICIT_TLS_PORT: 465,
  MCP_BIND_HOST: '127.0.0.1',
  DEFAULT_LOCAL_SERVER_URL: 'http://127.0.0.1:8080',
  DEFAULT_MCP_HTTP_PORT: 8765,
  SSE_RECONNECT_BASE_MS: 1000,
  SSE_RECONNECT_MAX_MS: 30000,
  SOURCE_EXPORT_FORMAT_VERSION: 1,
  PRESET_PACK_ID: 'gemini-ecosystem-v1',
  PRESET_PACK_NAME: 'Gemini Ecosystem Preset Pack',
  PRESET_CATEGORIES: ['Official', 'Releases', 'Community', 'News', 'Social'] as const,
  SECRET_MASK_REDACTED: '****',
  SECRET_MASK_CONFIGURED: 'configured',
  DEFAULT_OIDC_SCOPES: ['openid', 'profile', 'email'] as const,
} as const;

export const SSE_RUN_STAGES = {
  QUEUED: 'queued',
  COLLECTING: 'collecting',
  MATCHING: 'matching',
  SUMMARIZING: 'summarizing',
  RENDERING: 'rendering',
  NOTIFYING: 'notifying',
  COMPLETED: 'completed',
} as const;
export type SseRunStage = (typeof SSE_RUN_STAGES)[keyof typeof SSE_RUN_STAGES];

export const MCP_CLIENT_TARGETS = {
  GEMINI_CLI: 'gemini_cli',
  ANTIGRAVITY: 'antigravity',
  CHATGPT: 'chatgpt',
  CLAUDE_DESKTOP: 'claude_desktop',
  CODEX: 'codex',
} as const;
export type McpClientTarget = (typeof MCP_CLIENT_TARGETS)[keyof typeof MCP_CLIENT_TARGETS];

export const APP_INFO = {
  NAME: 'Gemini Daily Digest',
  DEFAULT_BASE_URL: '',
  getDefaultOrigin: (): string =>
    typeof window !== 'undefined' && typeof window.location?.origin === 'string'
      ? window.location.origin
      : '',
} as const;
