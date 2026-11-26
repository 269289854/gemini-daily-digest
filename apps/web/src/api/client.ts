/**
 * Strongly typed HTTP API client for `digest-server` REST endpoints.
 */

import { API_ROUTES, APP_INFO, HTTP_HEADERS, STORAGE_KEYS } from '../constants/routes';
import type {
  AiModelsRequest,
  AiModelsResult,
  AiTestRequest,
  AiTestResult,
  AuthConfigResponse,
  BookmarkView,
  ChannelTestResult,
  CreateChannelCmd,
  CreateSourceCmd,
  DailyDigestDetail,
  DailyDigestSummary,
  DeliveryLogRecord,
  DigestRunView,
  ErrorEnvelope,
  HealthResponse,
  ImportPresetRequest,
  ImportSourcesResponse,
  ItemSearchParams,
  ItemView,
  MeResponse,
  NotificationChannelView,
  PagedResult,
  PaginationParams,
  PlatformSettingsView,
  PlatformXTestRequest,
  PlatformXTestResult,
  RunStatsView,
  SourceDailyDigestView,
  SourceExportBundle,
  SourceTestResult,
  TriggerRunRequest,
  UpdateAiConfigCmd,
  UpdateChannelCmd,
  UpdatePlatformSettingsCmd,
  UpdatePlatformSettingsResult,
  UpdateScheduleCmd,
  UpdateSourceCmd,
  UpsertBookmarkCmd,
  UpsertWatchlistRuleCmd,
  UserAiConfigView,
  UserScheduleRecord,
  UserSourceView,
  WatchlistRuleRecord,
} from '../types/api';
import { ErrorCode } from '../types/enums';

export type QueryParamValue = string | number | boolean | null | undefined;
export type QueryParams = Record<string, QueryParamValue>;

export class ApiError extends Error {
  readonly status: number;
  readonly code: ErrorCode;
  readonly requestId?: string;
  readonly envelope?: ErrorEnvelope;

  constructor(
    message: string,
    status: number,
    code: ErrorCode,
    requestId?: string,
    envelope?: ErrorEnvelope,
  ) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
    this.requestId = requestId;
    this.envelope = envelope;
    Object.setPrototypeOf(this, ApiError.prototype);
  }
}

export interface ApiClientConfig {
  getBaseUrl: () => string;
  getAccessToken?: () => string | null;
  onUnauthorized?: () => void;
  fetchImpl?: typeof fetch;
}

const KNOWN_ERROR_CODES: ReadonlySet<string> = new Set(Object.values(ErrorCode));

function mapStatusToErrorCode(status: number): ErrorCode {
  switch (status) {
    case 400:
      return ErrorCode.INVALID_INPUT;
    case 401:
      return ErrorCode.UNAUTHORIZED;
    case 403:
      return ErrorCode.FORBIDDEN;
    case 404:
      return ErrorCode.NOT_FOUND;
    case 409:
      return ErrorCode.CONFLICT;
    case 422:
      return ErrorCode.SSRF_BLOCKED;
    case 502:
      return ErrorCode.UPSTREAM_ERROR;
    case 504:
      return ErrorCode.UPSTREAM_TIMEOUT;
    default:
      return ErrorCode.INTERNAL_ERROR;
  }
}

/**
 * Normalizes an optional server base URL:
 * - empty/whitespace returns `""` (relative same-origin)
 * - non-empty URL must be `http:` or `https:` and is returned without trailing slashes
 */
export function normalizeBaseUrl(url: string): string {
  const trimmed = url.trim();
  if (!trimmed) {
    return '';
  }
  const parsed = new URL(trimmed);
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw new Error('Server base URL must use http or https protocol');
  }
  return trimmed.replace(/\/+$/, '');
}

/**
 * Builds an API URL from base URL, route path, and optional query parameters.
 */
export function buildApiUrl(baseUrl: string, path: string, query?: QueryParams): string {
  const normalizedBase = normalizeBaseUrl(baseUrl);
  const normalizedPath = path.startsWith('/') ? path : `/${path}`;
  const search = new URLSearchParams();

  if (query) {
    for (const [key, value] of Object.entries(query)) {
      if (value === undefined || value === null) {
        continue;
      }
      if (typeof value === 'string' && value === '') {
        continue;
      }
      search.set(key, String(value));
    }
  }

  const qs = search.toString();
  return `${normalizedBase}${normalizedPath}${qs ? `?${qs}` : ''}`;
}

function isErrorEnvelope(value: unknown): value is ErrorEnvelope {
  if (!value || typeof value !== 'object') {
    return false;
  }
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.code === 'string' &&
    KNOWN_ERROR_CODES.has(candidate.code) &&
    typeof candidate.message === 'string'
  );
}

async function parseApiErrorResponse(response: Response): Promise<ApiError> {
  const headerRequestId = response.headers.get(HTTP_HEADERS.REQUEST_ID)?.trim() || undefined;
  let rawText = '';
  try {
    rawText = await response.text();
  } catch {
    rawText = '';
  }

  if (rawText) {
    try {
      const parsed: unknown = JSON.parse(rawText);
      if (isErrorEnvelope(parsed)) {
        const requestId =
          typeof parsed.requestId === 'string' && parsed.requestId.trim()
            ? parsed.requestId.trim()
            : headerRequestId;
        const envelope: ErrorEnvelope = {
          code: parsed.code,
          message: parsed.message,
          requestId: requestId ?? '',
        };
        return new ApiError(parsed.message, response.status, parsed.code, requestId, envelope);
      }
    } catch {
      // Fall through to status-based error construction when body is not JSON.
    }
  }

  const fallbackMessage = rawText.trim() || response.statusText || `HTTP ${response.status}`;
  return new ApiError(
    fallbackMessage,
    response.status,
    mapStatusToErrorCode(response.status),
    headerRequestId,
  );
}

interface RequestOptions {
  method: 'GET' | 'POST' | 'PUT' | 'DELETE';
  path: string;
  query?: QueryParams;
  body?: unknown;
  accept?: string;
  responseType?: 'json' | 'text' | 'void';
  skipAuth?: boolean;
}

export function createApiClient(config: ApiClientConfig) {
  const getFetcher = (): typeof fetch => {
    const impl = config.fetchImpl ?? globalThis.fetch;
    if (!impl) {
      throw new Error('Fetch implementation is not available in this environment');
    }
    return impl;
  };

  async function request<T>(opts: RequestOptions): Promise<T> {
    const url = buildApiUrl(config.getBaseUrl(), opts.path, opts.query);
    const headers = new Headers();
    headers.set(HTTP_HEADERS.ACCEPT, opts.accept ?? HTTP_HEADERS.CONTENT_TYPE_JSON);

    if (!opts.skipAuth) {
      const token = config.getAccessToken?.()?.trim();
      if (token) {
        headers.set(HTTP_HEADERS.AUTHORIZATION, `${HTTP_HEADERS.BEARER_PREFIX}${token}`);
      }
    }

    let payload: string | undefined;
    if (opts.body !== undefined) {
      headers.set(HTTP_HEADERS.CONTENT_TYPE, HTTP_HEADERS.CONTENT_TYPE_JSON);
      payload = JSON.stringify(opts.body);
    }

    const fetcher = getFetcher();
    const response = await fetcher(url, {
      method: opts.method,
      headers,
      body: payload,
    });

    if (!response.ok) {
      const apiErr = await parseApiErrorResponse(response);
      if (response.status === 401) {
        config.onUnauthorized?.();
      }
      throw apiErr;
    }

    if (opts.responseType === 'void' || response.status === 204) {
      return undefined as T;
    }

    if (opts.responseType === 'text') {
      return (await response.text()) as T;
    }

    const contentLength = response.headers.get('content-length');
    if (contentLength === '0') {
      return undefined as T;
    }

    return (await response.json()) as T;
  }

  return {
    // Health & Auth
    getHealth(): Promise<HealthResponse> {
      return request<HealthResponse>({
        method: 'GET',
        path: API_ROUTES.HEALTH,
        skipAuth: true,
      });
    },

    getAuthConfig(): Promise<AuthConfigResponse> {
      return request<AuthConfigResponse>({
        method: 'GET',
        path: API_ROUTES.AUTH_CONFIG,
        skipAuth: true,
      });
    },

    getMe(): Promise<MeResponse> {
      return request<MeResponse>({
        method: 'GET',
        path: API_ROUTES.ME,
      });
    },

    // Digests
    listDigests(params?: PaginationParams): Promise<PagedResult<DailyDigestSummary>> {
      return request<PagedResult<DailyDigestSummary>>({
        method: 'GET',
        path: API_ROUTES.DIGESTS,
        query: params ? { page: params.page, pageSize: params.pageSize } : undefined,
      });
    },

    getDigest(date: string): Promise<DailyDigestDetail> {
      return request<DailyDigestDetail>({
        method: 'GET',
        path: API_ROUTES.digestByDate(date),
      });
    },

    getDigestMarkdown(date: string): Promise<string> {
      return request<string>({
        method: 'GET',
        path: API_ROUTES.digestMarkdown(date),
        accept: 'text/markdown, text/plain',
        responseType: 'text',
      });
    },

    getSourceDigests(
      sourceId: string,
      params?: PaginationParams,
    ): Promise<PagedResult<SourceDailyDigestView>> {
      return request<PagedResult<SourceDailyDigestView>>({
        method: 'GET',
        path: API_ROUTES.sourceDigests(sourceId),
        query: params ? { page: params.page, pageSize: params.pageSize } : undefined,
      });
    },

    getSourceItems(sourceId: string, params?: PaginationParams): Promise<PagedResult<ItemView>> {
      return request<PagedResult<ItemView>>({
        method: 'GET',
        path: API_ROUTES.sourceItems(sourceId),
        query: params ? { page: params.page, pageSize: params.pageSize } : undefined,
      });
    },

    // Search & Bookmarks
    searchItems(params?: ItemSearchParams): Promise<PagedResult<ItemView>> {
      return request<PagedResult<ItemView>>({
        method: 'GET',
        path: API_ROUTES.ITEMS,
        query: params
          ? {
              q: params.q,
              sourceId: params.sourceId,
              dateFrom: params.dateFrom,
              dateTo: params.dateTo,
              keyword: params.keyword,
              bookmarked: params.bookmarked,
              page: params.page,
              pageSize: params.pageSize,
            }
          : undefined,
      });
    },

    upsertBookmark(cmd: UpsertBookmarkCmd): Promise<BookmarkView> {
      return request<BookmarkView>({
        method: 'POST',
        path: API_ROUTES.BOOKMARKS,
        body: cmd,
      });
    },

    deleteBookmark(itemId: string): Promise<void> {
      return request<void>({
        method: 'DELETE',
        path: API_ROUTES.bookmarkByItem(itemId),
        responseType: 'void',
      });
    },

    // Settings: AI
    getAiConfig(): Promise<UserAiConfigView> {
      return request<UserAiConfigView>({
        method: 'GET',
        path: API_ROUTES.SETTINGS_AI,
      });
    },

    updateAiConfig(cmd: UpdateAiConfigCmd): Promise<UserAiConfigView> {
      return request<UserAiConfigView>({
        method: 'PUT',
        path: API_ROUTES.SETTINGS_AI,
        body: cmd,
      });
    },

    testAiConfig(req?: AiTestRequest): Promise<AiTestResult> {
      return request<AiTestResult>({
        method: 'POST',
        path: API_ROUTES.SETTINGS_AI_TEST,
        body: req ?? {},
      });
    },

    listAiModels(req?: AiModelsRequest): Promise<AiModelsResult> {
      return request<AiModelsResult>({
        method: 'POST',
        path: API_ROUTES.SETTINGS_AI_MODELS,
        body: req ?? {},
      });
    },

    // Settings: Sources
    listSources(): Promise<UserSourceView[]> {
      return request<UserSourceView[]>({
        method: 'GET',
        path: API_ROUTES.SETTINGS_SOURCES,
      });
    },

    createSource(cmd: CreateSourceCmd): Promise<UserSourceView> {
      return request<UserSourceView>({
        method: 'POST',
        path: API_ROUTES.SETTINGS_SOURCES,
        body: cmd,
      });
    },

    updateSource(sourceId: string, cmd: UpdateSourceCmd): Promise<UserSourceView> {
      return request<UserSourceView>({
        method: 'PUT',
        path: API_ROUTES.sourceById(sourceId),
        body: cmd,
      });
    },

    deleteSource(sourceId: string): Promise<void> {
      return request<void>({
        method: 'DELETE',
        path: API_ROUTES.sourceById(sourceId),
        responseType: 'void',
      });
    },

    importPresetSources(req?: ImportPresetRequest): Promise<ImportSourcesResponse> {
      return request<ImportSourcesResponse>({
        method: 'POST',
        path: API_ROUTES.SETTINGS_SOURCES_IMPORT_PRESET,
        body: req ?? {},
      });
    },

    exportSources(): Promise<SourceExportBundle> {
      return request<SourceExportBundle>({
        method: 'POST',
        path: API_ROUTES.SETTINGS_SOURCES_EXPORT,
      });
    },

    importSources(bundle: SourceExportBundle): Promise<ImportSourcesResponse> {
      return request<ImportSourcesResponse>({
        method: 'POST',
        path: API_ROUTES.SETTINGS_SOURCES_IMPORT,
        body: bundle,
      });
    },

    testSource(sourceId: string): Promise<SourceTestResult> {
      return request<SourceTestResult>({
        method: 'POST',
        path: API_ROUTES.sourceTest(sourceId),
      });
    },

    // Settings: Notification Channels
    listChannels(): Promise<NotificationChannelView[]> {
      return request<NotificationChannelView[]>({
        method: 'GET',
        path: API_ROUTES.SETTINGS_CHANNELS,
      });
    },

    createChannel(cmd: CreateChannelCmd): Promise<NotificationChannelView> {
      return request<NotificationChannelView>({
        method: 'POST',
        path: API_ROUTES.SETTINGS_CHANNELS,
        body: cmd,
      });
    },

    updateChannel(channelId: string, cmd: UpdateChannelCmd): Promise<NotificationChannelView> {
      return request<NotificationChannelView>({
        method: 'PUT',
        path: API_ROUTES.channelById(channelId),
        body: cmd,
      });
    },

    deleteChannel(channelId: string): Promise<void> {
      return request<void>({
        method: 'DELETE',
        path: API_ROUTES.channelById(channelId),
        responseType: 'void',
      });
    },

    testChannel(channelId: string): Promise<ChannelTestResult> {
      return request<ChannelTestResult>({
        method: 'POST',
        path: API_ROUTES.channelTest(channelId),
      });
    },

    // Settings: Schedule & Watchlist
    getSchedule(): Promise<UserScheduleRecord> {
      return request<UserScheduleRecord>({
        method: 'GET',
        path: API_ROUTES.SETTINGS_SCHEDULE,
      });
    },

    updateSchedule(cmd: UpdateScheduleCmd): Promise<UserScheduleRecord> {
      return request<UserScheduleRecord>({
        method: 'PUT',
        path: API_ROUTES.SETTINGS_SCHEDULE,
        body: cmd,
      });
    },

    listWatchlist(): Promise<WatchlistRuleRecord[]> {
      return request<WatchlistRuleRecord[]>({
        method: 'GET',
        path: API_ROUTES.SETTINGS_WATCHLIST,
      });
    },

    createWatchlistRule(cmd: UpsertWatchlistRuleCmd): Promise<WatchlistRuleRecord> {
      return request<WatchlistRuleRecord>({
        method: 'POST',
        path: API_ROUTES.SETTINGS_WATCHLIST,
        body: cmd,
      });
    },

    updateWatchlistRule(
      ruleId: string,
      cmd: UpsertWatchlistRuleCmd,
    ): Promise<WatchlistRuleRecord> {
      return request<WatchlistRuleRecord>({
        method: 'PUT',
        path: API_ROUTES.watchlistById(ruleId),
        body: cmd,
      });
    },

    deleteWatchlistRule(ruleId: string): Promise<void> {
      return request<void>({
        method: 'DELETE',
        path: API_ROUTES.watchlistById(ruleId),
        responseType: 'void',
      });
    },

    // Runs & Deliveries
    triggerRun(req?: TriggerRunRequest): Promise<DigestRunView> {
      return request<DigestRunView>({
        method: 'POST',
        path: API_ROUTES.RUNS_TRIGGER,
        body: req ?? {},
      });
    },

    listRuns(params?: PaginationParams): Promise<PagedResult<DigestRunView>> {
      return request<PagedResult<DigestRunView>>({
        method: 'GET',
        path: API_ROUTES.RUNS,
        query: params ? { page: params.page, pageSize: params.pageSize } : undefined,
      });
    },

    getRunStats(): Promise<RunStatsView> {
      return request<RunStatsView>({
        method: 'GET',
        path: API_ROUTES.RUNS_STATS,
      });
    },

    resendDelivery(deliveryId: string): Promise<DeliveryLogRecord> {
      return request<DeliveryLogRecord>({
        method: 'POST',
        path: API_ROUTES.resendDelivery(deliveryId),
      });
    },

    // Admin Platform Settings
    getAdminPlatformSettings(): Promise<PlatformSettingsView> {
      return request<PlatformSettingsView>({
        method: 'GET',
        path: API_ROUTES.ADMIN_PLATFORM,
      });
    },

    updateAdminPlatformSettings(
      cmd: UpdatePlatformSettingsCmd,
    ): Promise<UpdatePlatformSettingsResult> {
      return request<UpdatePlatformSettingsResult>({
        method: 'PUT',
        path: API_ROUTES.ADMIN_PLATFORM,
        body: cmd,
      });
    },

    testAdminPlatformX(req?: PlatformXTestRequest): Promise<PlatformXTestResult> {
      return request<PlatformXTestResult>({
        method: 'POST',
        path: API_ROUTES.ADMIN_PLATFORM_X_TEST,
        body: req ?? {},
      });
    },
  };
}

export type ApiClient = ReturnType<typeof createApiClient>;

function safeReadStorage(key: string): string | null {
  if (typeof window === 'undefined') {
    return null;
  }
  try {
    const localVal = window.localStorage?.getItem(key);
    if (localVal) {
      return localVal;
    }
    return window.sessionStorage?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

export const apiClient: ApiClient = createApiClient({
  getBaseUrl: () => {
    const stored = safeReadStorage(STORAGE_KEYS.SERVER_BASE_URL);
    if (!stored) {
      return APP_INFO.DEFAULT_BASE_URL;
    }
    try {
      return normalizeBaseUrl(stored);
    } catch {
      return APP_INFO.DEFAULT_BASE_URL;
    }
  },
  getAccessToken: () => safeReadStorage(STORAGE_KEYS.ACCESS_TOKEN),
});
