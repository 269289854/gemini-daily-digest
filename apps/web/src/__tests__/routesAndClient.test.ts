import { describe, expect, it, vi } from 'vitest';
import { ApiError, buildApiUrl, createApiClient, normalizeBaseUrl } from '../api/client';
import { API_ROUTES, HTTP_HEADERS } from '../constants/routes';
import { ErrorCode } from '../types/enums';

function jsonResponse(body: unknown, status = 200, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      'content-type': 'application/json',
      ...headers,
    },
  });
}

describe('API_ROUTES, URL helpers, and ApiClient', () => {
  it('builds encoded dynamic routes across all API_ROUTES helpers', () => {
    expect(API_ROUTES.sourceById('src/1?a=b')).toBe('/api/v1/settings/sources/src%2F1%3Fa%3Db');
    expect(API_ROUTES.sourceTest('src 1')).toBe('/api/v1/settings/sources/src%201/test');
    expect(API_ROUTES.channelById('ch/1')).toBe('/api/v1/settings/channels/ch%2F1');
    expect(API_ROUTES.channelTest('ch/1')).toBe('/api/v1/settings/channels/ch%2F1/test');
    expect(API_ROUTES.watchlistById('wl#1')).toBe('/api/v1/settings/watchlist/wl%231');
    expect(API_ROUTES.digestByDate('2026-11-26')).toBe('/api/v1/digests/2026-11-26');
    expect(API_ROUTES.digestMarkdown('2026-11-26')).toBe('/api/v1/digests/2026-11-26/markdown');
    expect(API_ROUTES.sourceDigests('s1')).toBe('/api/v1/sources/s1/digests');
    expect(API_ROUTES.sourceItems('s1')).toBe('/api/v1/sources/s1/items');
    expect(API_ROUTES.bookmarkByItem('item-1')).toBe('/api/v1/bookmarks/item-1');
    expect(API_ROUTES.resendDelivery('del-1')).toBe('/api/v1/deliveries/del-1/resend');
    expect(API_ROUTES.EVENTS_STREAM).toBe('/api/v1/events/stream');
  });

  it('normalizes server base URLs and rejects unsafe schemes', () => {
    expect(normalizeBaseUrl('')).toBe('');
    expect(normalizeBaseUrl('   ')).toBe('');
    expect(normalizeBaseUrl('https://digest.example.com///')).toBe('https://digest.example.com');
    expect(normalizeBaseUrl('http://127.0.0.1:8080/')).toBe('http://127.0.0.1:8080');

    expect(() => normalizeBaseUrl('javascript:alert(1)')).toThrow();
    expect(() => normalizeBaseUrl('ftp://files.example.com')).toThrow();
    expect(() => normalizeBaseUrl('not a url')).toThrow();
  });

  it('builds API URLs with clean query parameter filtering', () => {
    expect(buildApiUrl('', API_ROUTES.ITEMS)).toBe('/api/v1/items');
    expect(
      buildApiUrl('https://digest.example.com/', 'api/v1/items', {
        q: 'gemini',
        sourceId: '',
        dateFrom: undefined,
        dateTo: null,
        bookmarked: true,
        page: 2,
      }),
    ).toBe('https://digest.example.com/api/v1/items?q=gemini&bookmarked=true&page=2');
  });

  it('attaches Bearer auth only on protected routes and reads x-request-id on errors', async () => {
    const fetchMock = vi.fn<typeof fetch>();
    fetchMock
      .mockResolvedValueOnce(jsonResponse({ status: 'ok', service: 'digest-server' }))
      .mockResolvedValueOnce(
        jsonResponse({
          user: {
            id: 'u1',
            oidcSub: 'sub-1',
            email: 'reader@example.com',
            displayName: 'Reader',
            role: 'user',
            createdAt: '2026-11-26T00:00:00Z',
            lastLoginAt: null,
          },
          onboarding: {
            hasAiConfig: true,
            hasSources: true,
            hasChannels: false,
            hasSchedule: true,
          },
        }),
      );

    const client = createApiClient({
      getBaseUrl: () => 'https://digest.example.com/',
      getAccessToken: () => 'test-tok-123',
      fetchImpl: fetchMock,
    });

    const health = await client.getHealth();
    expect(health.status).toBe('ok');
    const healthHeaders = fetchMock.mock.calls[0][1]?.headers as Headers;
    expect(healthHeaders.get(HTTP_HEADERS.AUTHORIZATION)).toBeNull();

    const me = await client.getMe();
    expect(me.user.email).toBe('reader@example.com');
    const meHeaders = fetchMock.mock.calls[1][1]?.headers as Headers;
    expect(meHeaders.get(HTTP_HEADERS.AUTHORIZATION)).toBe('Bearer test-tok-123');
    expect(fetchMock.mock.calls[1][0]).toBe('https://digest.example.com/api/v1/me');
  });

  it('handles 204 No Content delete responses and text/markdown responses', async () => {
    const fetchMock = vi.fn<typeof fetch>();
    fetchMock
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
      .mockResolvedValueOnce(
        new Response('# Gemini Daily Digest\n\n- Update 1', {
          status: 200,
          headers: { 'content-type': 'text/markdown; charset=utf-8' },
        }),
      );

    const client = createApiClient({
      getBaseUrl: () => '',
      getAccessToken: () => 'test-tok-123',
      fetchImpl: fetchMock,
    });

    await expect(client.deleteSource('src-1')).resolves.toBeUndefined();
    await expect(client.deleteChannel('ch-1')).resolves.toBeUndefined();
    await expect(client.deleteWatchlistRule('wl-1')).resolves.toBeUndefined();
    await expect(client.deleteBookmark('item-1')).resolves.toBeUndefined();

    expect(fetchMock.mock.calls[0][1]?.method).toBe('DELETE');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/v1/settings/sources/src-1');

    const md = await client.getDigestMarkdown('2026-11-26');
    expect(md).toContain('# Gemini Daily Digest');
    const mdHeaders = fetchMock.mock.calls[4][1]?.headers as Headers;
    expect(mdHeaders.get(HTTP_HEADERS.ACCEPT)).toBe('text/markdown, text/plain');
  });

  it('parses ErrorEnvelope into ApiError with x-request-id fallback and triggers onUnauthorized on 401', async () => {
    const onUnauthorized = vi.fn();
    const fetchMock = vi.fn<typeof fetch>();
    fetchMock
      .mockResolvedValueOnce(
        jsonResponse(
          {
            code: ErrorCode.SSRF_BLOCKED,
            message: 'Private network URL blocked',
            requestId: '',
          },
          422,
          { [HTTP_HEADERS.REQUEST_ID]: 'req-hdr-99' },
        ),
      )
      .mockResolvedValueOnce(
        jsonResponse(
          {
            code: ErrorCode.UNAUTHORIZED,
            message: 'Token expired',
            requestId: 'req-body-401',
          },
          401,
        ),
      )
      .mockResolvedValueOnce(
        new Response('Gateway Timeout', {
          status: 504,
          statusText: 'Gateway Timeout',
          headers: { [HTTP_HEADERS.REQUEST_ID]: 'req-504' },
        }),
      );

    const client = createApiClient({
      getBaseUrl: () => '',
      getAccessToken: () => 'test-tok-123',
      onUnauthorized,
      fetchImpl: fetchMock,
    });

    await expect(client.listSources()).rejects.toMatchObject({
      name: 'ApiError',
      status: 422,
      code: ErrorCode.SSRF_BLOCKED,
      message: 'Private network URL blocked',
      requestId: 'req-hdr-99',
    });
    expect(onUnauthorized).not.toHaveBeenCalled();

    await expect(client.getMe()).rejects.toSatisfy((err: unknown) => {
      return (
        err instanceof ApiError &&
        err.status === 401 &&
        err.code === ErrorCode.UNAUTHORIZED &&
        err.requestId === 'req-body-401'
      );
    });
    expect(onUnauthorized).toHaveBeenCalledTimes(1);

    await expect(client.getRunStats()).rejects.toMatchObject({
      status: 504,
      code: ErrorCode.UPSTREAM_TIMEOUT,
      requestId: 'req-504',
    });
  });

  it('retries once on 401 when refreshAccessToken yields a fresh token, and calls onUnauthorized only when refresh fails or retry still returns 401', async () => {
    let activeToken = 'expired-tok';
    const refreshAccessToken = vi.fn(async () => {
      activeToken = 'refreshed-tok';
      return activeToken;
    });
    const onUnauthorized = vi.fn();
    const fetchMock = vi.fn<typeof fetch>(async (_input, init) => {
      const headers = init?.headers as Headers;
      const auth = headers.get(HTTP_HEADERS.AUTHORIZATION);
      if (auth === 'Bearer refreshed-tok') {
        return jsonResponse({
          user: {
            id: 'u1',
            oidcSub: 'sub-1',
            email: 'reader@example.com',
            displayName: 'Reader',
            role: 'user',
            createdAt: '2026-11-26T00:00:00Z',
            lastLoginAt: null,
          },
          onboarding: {
            hasAiConfig: true,
            hasSources: true,
            hasChannels: false,
            hasSchedule: true,
          },
        });
      }
      return jsonResponse({ code: ErrorCode.UNAUTHORIZED, message: 'Expired', requestId: 'r1' }, 401);
    });

    const client = createApiClient({
      getBaseUrl: () => '',
      getAccessToken: () => activeToken,
      refreshAccessToken,
      onUnauthorized,
      fetchImpl: fetchMock,
    });

    const me = await client.getMe();
    expect(me.user.email).toBe('reader@example.com');
    expect(refreshAccessToken).toHaveBeenCalledTimes(1);
    expect(onUnauthorized).not.toHaveBeenCalled();
    expect(fetchMock).toHaveBeenCalledTimes(2);

    // Case 2: refresh succeeds with token, but server still returns 401 -> no infinite loop, triggers onUnauthorized once
    const always401Fetch = vi.fn<typeof fetch>(async () =>
      jsonResponse({ code: ErrorCode.UNAUTHORIZED, message: 'Still rejected', requestId: 'r2' }, 401),
    );
    const refreshAgain = vi.fn().mockResolvedValue('another-tok');
    const onUnauth2 = vi.fn();
    const client2 = createApiClient({
      getBaseUrl: () => '',
      getAccessToken: () => 'old-tok',
      refreshAccessToken: refreshAgain,
      onUnauthorized: onUnauth2,
      fetchImpl: always401Fetch,
    });

    await expect(client2.getMe()).rejects.toMatchObject({ status: 401 });
    expect(refreshAgain).toHaveBeenCalledTimes(1);
    expect(always401Fetch).toHaveBeenCalledTimes(2);
    expect(onUnauth2).toHaveBeenCalledTimes(1);
  });
});
