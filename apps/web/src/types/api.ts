/**
 * 1:1 TypeScript DTO contracts mirroring `crates/digest-server/src/repo/dto.rs`,
 * `crates/digest-core/src/domain/models.rs`, `crates/digest-server/src/api/*`,
 * `crates/digest-server/src/services.rs`, and `crates/digest-server/src/sse_hub.rs`.
 */

import type { SseRunStage } from '../constants/routes';
import type {
  AlertPriority,
  AuthMode,
  DeliveryStatus,
  ErrorCode,
  FetchOutcome,
  NotificationChannelType,
  RunStatus,
  RunType,
  SourceScope,
  SourceType,
  SseEventType,
} from './enums';

// ============================================================================
// Common & Error Envelope
// ============================================================================

export interface ErrorEnvelope {
  code: ErrorCode;
  message: string;
  requestId: string;
}

export interface HealthResponse {
  status: string;
  service: string;
}

export interface PagedResult<T> {
  items: T[];
  page: number;
  pageSize: number;
  total: number;
  totalPages: number;
}

export interface PaginationParams {
  page?: number;
  pageSize?: number;
}

// ============================================================================
// Auth & Profile
// ============================================================================

export interface AuthConfigResponse {
  authMode: AuthMode;
  issuerUrl: string | null;
  clientId: string | null;
  scopes: string[];
}

export interface UserRecord {
  id: string;
  oidcSub: string;
  email: string | null;
  displayName: string | null;
  role: string;
  createdAt: string;
  lastLoginAt: string | null;
}

export interface OnboardingFlags {
  hasAiConfig: boolean;
  hasSources: boolean;
  hasChannels: boolean;
  hasSchedule: boolean;
}

export interface MeResponse {
  user: UserRecord;
  onboarding: OnboardingFlags;
}

// ============================================================================
// Digests & Processed Items
// ============================================================================

export interface ItemMatchResult {
  matchedKeywords: string[];
  priority: AlertPriority;
  importanceScore: number;
  notifyImmediately: boolean;
}

export interface ProcessedItem {
  externalId: string | null;
  canonicalUrl: string;
  contentHash: string;
  title: string;
  author: string | null;
  summary: string | null;
  publishedAt: string | null;
  sourceId: string | null;
  sourceName: string | null;
  category: string | null;
  matchResult: ItemMatchResult;
}

export interface DigestSection {
  category: string;
  sourceId: string | null;
  sourceName: string;
  summaryMarkdown: string;
  keyPoints: string[];
  items: ProcessedItem[];
}

export interface DailyDigestSummary {
  id: string;
  digestDate: string;
  title: string;
  itemCount: number;
  highlightedCount: number;
  promptTokens: number;
  completionTokens: number;
  modelUsed: string;
  degradedFallback: boolean;
  createdAt: string;
}

export interface DailyDigestDetail extends DailyDigestSummary {
  inputFingerprint: string;
  overviewMarkdown: string;
  overviewHtml: string;
  sections: DigestSection[];
}

export interface SourceDailyDigestView {
  id: string;
  digestId: string | null;
  sourceId: string;
  sourceName: string;
  digestDate: string;
  inputFingerprint: string;
  summaryMarkdown: string;
  keyPoints: string[];
  itemCount: number;
  createdAt: string;
}

// ============================================================================
// Items, Search & Bookmarks
// ============================================================================

export interface ItemView {
  id: string;
  sourceId: string;
  sourceName: string;
  category: string;
  externalId: string | null;
  canonicalUrl: string;
  title: string;
  author: string | null;
  summary: string | null;
  bodyPruned: boolean;
  publishedAt: string | null;
  fetchedAt: string;
  matchedKeywords: string[];
  priority: AlertPriority;
  importanceScore: number;
  bookmarked: boolean;
  bookmarkNote: string | null;
}

export interface ItemSearchParams extends PaginationParams {
  q?: string;
  sourceId?: string;
  dateFrom?: string;
  dateTo?: string;
  keyword?: string;
  bookmarked?: boolean;
}

export interface BookmarkView {
  id: string;
  itemId: string;
  note: string | null;
  createdAt: string;
}

export interface UpsertBookmarkCmd {
  itemId: string;
  note?: string | null;
}

// ============================================================================
// Settings: AI Config
// ============================================================================

export interface UserAiConfigView {
  providerLabel: string;
  baseUrl: string;
  apiKeyHint: string | null;
  hasApiKey: boolean;
  model: string;
  systemPrompt: string;
  temperature: number;
  maxTokens: number;
  updatedAt: string | null;
}

export interface UpdateAiConfigCmd {
  providerLabel: string;
  baseUrl: string;
  apiKey?: string | null;
  model: string;
  systemPrompt?: string | null;
  temperature?: number | null;
  maxTokens?: number | null;
  clearApiKey?: boolean;
}

export interface AiTestRequest {
  providerLabel?: string | null;
  baseUrl?: string | null;
  apiKey?: string | null;
  model?: string | null;
  systemPrompt?: string | null;
  temperature?: number | null;
  maxTokens?: number | null;
}

export interface AiTestResult {
  ok: boolean;
  providerLabel: string;
  model: string;
  latencyMs: number;
  message: string;
}

// ============================================================================
// Settings: Sources, Presets & Export/Import
// ============================================================================

export interface SourceCredentialsPayload {
  bearerToken?: string;
  accessToken?: string;
  token?: string;
  apiKey?: string;
  apiBaseUrl?: string;
  [key: string]: unknown;
}

export interface UserSourceView {
  id: string;
  subscriptionId: string;
  sourceType: SourceType;
  scope: SourceScope;
  canonicalKey: string;
  customName: string;
  defaultName: string;
  category: string;
  handleOrUrl: string;
  enabled: boolean;
  hasCredentials: boolean;
  configHint: string | null;
  lastFetchedAt: string | null;
  nextPollAt: string | null;
  lastStatus: FetchOutcome | null;
  lastError: string | null;
  consecutiveFailures: number;
  createdAt: string;
}

export interface CreateSourceCmd {
  sourceType: SourceType;
  scope?: SourceScope | null;
  canonicalKey?: string | null;
  customName?: string | null;
  defaultName?: string | null;
  category?: string | null;
  handleOrUrl: string;
  credentials?: SourceCredentialsPayload | null;
  enabled?: boolean;
}

export interface UpdateSourceCmd {
  customName?: string | null;
  category?: string | null;
  enabled?: boolean | null;
  handleOrUrl?: string | null;
  credentials?: SourceCredentialsPayload | null;
  clearCredentials?: boolean;
}

export interface ImportPresetRequest {
  canonicalKeys?: string[] | null;
  includeXTemplates?: boolean;
}

export interface ImportSourcesResponse {
  importedCount: number;
  sources: UserSourceView[];
}

export interface ExportedSourceSpec {
  sourceType: SourceType;
  scope: SourceScope;
  canonicalKey: string;
  customName: string;
  category: string;
  handleOrUrl: string;
  enabled: boolean;
}

export interface SourceExportBundle {
  version: number;
  exportedAt: string;
  sources: ExportedSourceSpec[];
}

export interface SourceTestResult {
  sourceId: string;
  outcome: FetchOutcome;
  itemsFetched: number;
  sampleTitles: string[];
  errorMessage: string | null;
}

// ============================================================================
// Settings: Notification Channels
// ============================================================================

export interface SmtpChannelConfig {
  host: string;
  port?: number;
  username?: string | null;
  password?: string | null;
  from: string;
  to: string[] | string;
  useStarttls?: boolean | null;
  useTls?: boolean | null;
  allowInsecurePlain?: boolean | null;
}

export interface TelegramChannelConfig {
  botToken: string;
  chatId: string;
  apiBaseUrl?: string | null;
}

export interface FeishuChannelConfig {
  webhookUrl: string;
  secret?: string | null;
}

export interface DingTalkChannelConfig {
  webhookUrl: string;
  secret?: string | null;
}

export interface WeComChannelConfig {
  webhookUrl: string;
}

export interface WebhookChannelConfig {
  url: string;
  secret?: string | null;
  headers?: Record<string, string>;
}

export type ChannelConfigPayload =
  | SmtpChannelConfig
  | TelegramChannelConfig
  | FeishuChannelConfig
  | DingTalkChannelConfig
  | WeComChannelConfig
  | WebhookChannelConfig;

export interface NotificationChannelView {
  id: string;
  channelType: NotificationChannelType;
  name: string;
  enabled: boolean;
  configHint: string;
  triggerOnDaily: boolean;
  triggerOnAlert: boolean;
  createdAt: string;
}

export interface CreateChannelCmd {
  channelType: NotificationChannelType;
  name: string;
  enabled?: boolean;
  config: ChannelConfigPayload;
  triggerOnDaily?: boolean;
  triggerOnAlert?: boolean;
}

export interface UpdateChannelCmd {
  name?: string | null;
  enabled?: boolean | null;
  config?: ChannelConfigPayload | null;
  triggerOnDaily?: boolean | null;
  triggerOnAlert?: boolean | null;
}

export interface ChannelTestResult {
  channelId: string;
  channelType: NotificationChannelType;
  status: DeliveryStatus;
  errorMessage: string | null;
  sentAt: string;
}

// ============================================================================
// Settings: Schedule & Watchlist
// ============================================================================

export interface UserScheduleRecord {
  userId: string;
  timezone: string;
  dailyTime: string;
  emailEnabled: boolean;
  sendWhenEmpty: boolean;
  alertScanEnabled: boolean;
  alertIntervalMinutes: number;
  lastDailyRunDate: string | null;
  lastAlertScanAt: string | null;
}

export interface UpdateScheduleCmd {
  timezone: string;
  dailyTime: string;
  emailEnabled: boolean;
  sendWhenEmpty: boolean;
  alertScanEnabled: boolean;
  alertIntervalMinutes: number;
}

export interface WatchlistRuleRecord {
  id: string;
  userId: string;
  name: string;
  keywords: string[];
  excludeKeywords: string[];
  priority: AlertPriority;
  notifyImmediately: boolean;
  enabled: boolean;
  createdAt: string;
}

export interface UpsertWatchlistRuleCmd {
  name: string;
  keywords: string[];
  excludeKeywords: string[];
  priority: AlertPriority;
  notifyImmediately: boolean;
  enabled: boolean;
}

// ============================================================================
// Runs, Deliveries & Stats
// ============================================================================

export interface DeliveryLogRecord {
  id: string;
  runId: string;
  userId: string;
  digestId: string | null;
  channelId: string | null;
  channelType: NotificationChannelType;
  status: DeliveryStatus;
  errorMessage: string | null;
  sentAt: string;
}

export interface DigestRunView {
  id: string;
  runType: RunType;
  status: RunStatus;
  startedAt: string;
  finishedAt: string | null;
  itemsFetched: number;
  itemsNew: number;
  promptTokens: number;
  completionTokens: number;
  errorSummary: string | null;
  deliveries: DeliveryLogRecord[];
}

export interface SourceHealthStat {
  sourceId: string;
  sourceName: string;
  sourceType: SourceType;
  scope: SourceScope;
  enabled: boolean;
  lastStatus: FetchOutcome | null;
  lastFetchedAt: string | null;
  consecutiveFailures: number;
  lastError: string | null;
}

export interface RunStatsView {
  totalRuns: number;
  succeededRuns: number;
  failedRuns: number;
  totalPromptTokens: number;
  totalCompletionTokens: number;
  deliveriesSent: number;
  deliveriesFailed: number;
  sourceHealth: SourceHealthStat[];
}

export interface TriggerRunRequest {
  digestDate?: string | null;
  forceRefresh?: boolean;
  notify?: boolean;
}

// ============================================================================
// SSE Events
// ============================================================================

export interface RunProgressEvent {
  runId: string;
  runType: RunType;
  status: RunStatus;
  stage: SseRunStage;
  itemsFetched: number;
  itemsNew: number;
  message?: string;
}

export interface DigestReadyEvent {
  digestId: string;
  digestDate: string;
  title: string;
  itemCount: number;
  highlightedCount: number;
  degradedFallback: boolean;
}

export interface AlertTriggeredItem {
  itemId: string;
  sourceId: string;
  sourceName: string;
  title: string;
  canonicalUrl: string;
  priority: AlertPriority;
  matchedKeywords: string[];
  publishedAt: string | null;
}

export interface AlertTriggeredEvent {
  runId: string;
  count: number;
  items: AlertTriggeredItem[];
}

export interface HeartbeatEvent {
  timestamp: string;
}

export interface SseEventMap {
  [SseEventType.RUN_PROGRESS]: RunProgressEvent;
  [SseEventType.DIGEST_READY]: DigestReadyEvent;
  [SseEventType.ALERT_TRIGGERED]: AlertTriggeredEvent;
  [SseEventType.HEARTBEAT]: HeartbeatEvent;
}
