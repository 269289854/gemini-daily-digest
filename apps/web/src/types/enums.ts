/**
 * 1:1 TypeScript mirror of `digest_core::domain::enums`.
 * All UI views and API clients MUST reference these constants instead of raw strings.
 */

export const SourceType = {
  RSS: 'rss',
  GITHUB_RELEASE: 'github_release',
  REDDIT: 'reddit',
  HACKER_NEWS: 'hacker_news',
  GOOGLE_NEWS: 'google_news',
  X_ACCOUNT: 'x_account',
  X_SEARCH: 'x_search',
} as const;
export type SourceType = (typeof SourceType)[keyof typeof SourceType];

export const SourceScope = {
  SHARED_PUBLIC: 'shared_public',
  USER_PRIVATE: 'user_private',
} as const;
export type SourceScope = (typeof SourceScope)[keyof typeof SourceScope];

export const FetchOutcome = {
  UPDATED: 'updated',
  NOT_MODIFIED_304: 'not_modified_304',
  SKIPPED_FRESH: 'skipped_fresh',
  FAILED: 'failed',
} as const;
export type FetchOutcome = (typeof FetchOutcome)[keyof typeof FetchOutcome];

export const NotificationChannelType = {
  SMTP: 'smtp',
  TELEGRAM: 'telegram',
  FEISHU: 'feishu',
  DINGTALK: 'dingtalk',
  WECOM: 'wecom',
  WEBHOOK: 'webhook',
} as const;
export type NotificationChannelType =
  (typeof NotificationChannelType)[keyof typeof NotificationChannelType];

export const RunType = {
  SCHEDULED_DAILY: 'scheduled_daily',
  MANUAL_DAILY: 'manual_daily',
  ALERT_SCAN: 'alert_scan',
} as const;
export type RunType = (typeof RunType)[keyof typeof RunType];

export const RunStatus = {
  RUNNING: 'running',
  SUCCEEDED: 'succeeded',
  PARTIAL: 'partial',
  FAILED: 'failed',
} as const;
export type RunStatus = (typeof RunStatus)[keyof typeof RunStatus];

export const DeliveryStatus = {
  PENDING: 'pending',
  SENT: 'sent',
  FAILED: 'failed',
} as const;
export type DeliveryStatus = (typeof DeliveryStatus)[keyof typeof DeliveryStatus];

export const AlertPriority = {
  NORMAL: 'normal',
  HIGH: 'high',
  URGENT: 'urgent',
} as const;
export type AlertPriority = (typeof AlertPriority)[keyof typeof AlertPriority];

export const McpAccessMode = {
  DISABLED: 'disabled',
  READ_ONLY: 'read_only',
  READ_WRITE: 'read_write',
} as const;
export type McpAccessMode = (typeof McpAccessMode)[keyof typeof McpAccessMode];

export const AuthMode = {
  OIDC: 'oidc',
  DEV_BYPASS: 'dev_bypass',
} as const;
export type AuthMode = (typeof AuthMode)[keyof typeof AuthMode];

export const SseEventType = {
  RUN_PROGRESS: 'run_progress',
  DIGEST_READY: 'digest_ready',
  ALERT_TRIGGERED: 'alert_triggered',
  HEARTBEAT: 'heartbeat',
} as const;
export type SseEventType = (typeof SseEventType)[keyof typeof SseEventType];

export const ErrorCode = {
  INVALID_INPUT: 'INVALID_INPUT',
  UNAUTHORIZED: 'UNAUTHORIZED',
  FORBIDDEN: 'FORBIDDEN',
  NOT_FOUND: 'NOT_FOUND',
  CONFLICT: 'CONFLICT',
  SSRF_BLOCKED: 'SSRF_BLOCKED',
  CRYPTO_ERROR: 'CRYPTO_ERROR',
  UPSTREAM_TIMEOUT: 'UPSTREAM_TIMEOUT',
  UPSTREAM_ERROR: 'UPSTREAM_ERROR',
  DATABASE_ERROR: 'DATABASE_ERROR',
  INTERNAL_ERROR: 'INTERNAL_ERROR',
} as const;
export type ErrorCode = (typeof ErrorCode)[keyof typeof ErrorCode];

export const Locale = {
  ZH_CN: 'zh-CN',
  EN_US: 'en-US',
} as const;
export type Locale = (typeof Locale)[keyof typeof Locale];

export const McpToolName = {
  DIGEST_GET_DAILY: 'digest_get_daily',
  DIGEST_LIST_DATES: 'digest_list_dates',
  DIGEST_LIST_SOURCES: 'digest_list_sources',
  DIGEST_GET_SOURCE_DIGESTS: 'digest_get_source_digests',
  DIGEST_SEARCH_ITEMS: 'digest_search_items',
  DIGEST_GET_BOOKMARKS: 'digest_get_bookmarks',
  DIGEST_GET_RUNS_STATS: 'digest_get_runs_stats',
  DIGEST_TRIGGER_RUN: 'digest_trigger_run',
  DIGEST_ADD_SOURCE: 'digest_add_source',
  DIGEST_TOGGLE_SOURCE: 'digest_toggle_source',
  DIGEST_BOOKMARK_ITEM: 'digest_bookmark_item',
  DIGEST_UPDATE_SCHEDULE: 'digest_update_schedule',
  DIGEST_ADD_WATCHLIST_RULE: 'digest_add_watchlist_rule',
  DIGEST_OPEN_DESKTOP_VIEW: 'digest_open_desktop_view',
} as const;
export type McpToolName = (typeof McpToolName)[keyof typeof McpToolName];
