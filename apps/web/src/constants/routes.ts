/**
 * Centralized API paths, frontend routes, and client storage keys.
 */

export const API_ROUTES = {
  HEALTH: '/health',
  AUTH_CONFIG: '/api/v1/auth/config',
  ME: '/api/v1/me',
  DIGESTS: '/api/v1/digests',
  digestByDate: (date: string) => `/api/v1/digests/${encodeURIComponent(date)}`,
  digestMarkdown: (date: string) => `/api/v1/digests/${encodeURIComponent(date)}/markdown`,
  sourceDigests: (sourceId: string) => `/api/v1/sources/${encodeURIComponent(sourceId)}/digests`,
  sourceItems: (sourceId: string) => `/api/v1/sources/${encodeURIComponent(sourceId)}/items`,
  ITEMS: '/api/v1/items',
  BOOKMARKS: '/api/v1/bookmarks',
  bookmarkByItem: (itemId: string) => `/api/v1/bookmarks/${encodeURIComponent(itemId)}`,
  SETTINGS_AI: '/api/v1/settings/ai',
  SETTINGS_AI_TEST: '/api/v1/settings/ai/test',
  SETTINGS_SOURCES: '/api/v1/settings/sources',
  SETTINGS_SOURCES_IMPORT_PRESET: '/api/v1/settings/sources/import-preset',
  SETTINGS_SOURCES_EXPORT: '/api/v1/settings/sources/export',
  SETTINGS_SOURCES_IMPORT: '/api/v1/settings/sources/import',
  sourceTest: (sourceId: string) => `/api/v1/settings/sources/${encodeURIComponent(sourceId)}/test`,
  SETTINGS_CHANNELS: '/api/v1/settings/channels',
  channelTest: (channelId: string) => `/api/v1/settings/channels/${encodeURIComponent(channelId)}/test`,
  SETTINGS_SCHEDULE: '/api/v1/settings/schedule',
  SETTINGS_WATCHLIST: '/api/v1/settings/watchlist',
  RUNS_TRIGGER: '/api/v1/runs/trigger',
  RUNS: '/api/v1/runs',
  RUNS_STATS: '/api/v1/runs/stats',
  resendDelivery: (deliveryId: string) => `/api/v1/deliveries/${encodeURIComponent(deliveryId)}/resend`,
  EVENTS_STREAM: '/api/v1/events/stream',
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
} as const;
