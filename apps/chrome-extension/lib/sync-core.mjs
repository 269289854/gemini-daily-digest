/**
 * Pure ESM core logic for the Gemini Daily Digest Admin X Cookie Sync extension.
 * Contains zero Chrome runtime dependencies so it can be unit tested under Node.js.
 */

export const STORAGE_KEYS = {
  ACCESS_TOKEN: 'gemini_digest.access_token',
  REFRESH_TOKEN: 'gemini_digest.refresh_token',
  TOKEN_EXPIRES_AT: 'gemini_digest.token_expires_at',
};

export const API_ROUTES = {
  AUTH_CONFIG: '/api/v1/auth/config',
  ME: '/api/v1/me',
  ADMIN_PLATFORM: '/api/v1/admin/platform',
  ADMIN_PLATFORM_X_TEST: '/api/v1/admin/platform/x/test',
};

export class ApiHttpError extends Error {
  /**
   * @param {string} message
   * @param {number} status
   * @param {string} [code]
   */
  constructor(message, status, code = 'HTTP_ERROR') {
    super(message);
    this.name = 'ApiHttpError';
    this.status = status;
    this.code = code;
  }
}

/**
 * Validates and normalizes the target Gemini Daily Digest server URL.
 * Returns `{ baseUrl, origin, originPattern }`.
 *
 * @param {string} raw
 */
export function normalizeServerUrl(raw) {
  const trimmed = typeof raw === 'string' ? raw.trim() : '';
  if (!trimmed) {
    throw new Error('Server URL is required');
  }
  const hasExplicitScheme = /^[a-z][a-z0-9+.-]*:/i.test(trimmed);
  const withScheme = hasExplicitScheme ? trimmed : `https://${trimmed}`;
  const parsed = new URL(withScheme);
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw new Error('Server URL must use http or https protocol');
  }
  const baseUrl = `${parsed.origin}${parsed.pathname}`.replace(/\/+$/, '');
  return {
    baseUrl,
    origin: parsed.origin,
    originPattern: `${parsed.origin}/*`,
  };
}

/**
 * Masks sensitive tokens so raw secrets are never rendered in UI or logs.
 *
 * @param {string | null | undefined} token
 */
export function maskToken(token) {
  const trimmed = typeof token === 'string' ? token.trim() : '';
  if (!trimmed) {
    return '****';
  }
  if (trimmed.length <= 10) {
    return `${trimmed.slice(0, 2)}****`;
  }
  return `${trimmed.slice(0, 4)}****${trimmed.slice(-4)}`;
}

/**
 * Safely decodes the `exp` claim from a standard 3-segment JWT into epoch milliseconds.
 * Returns `null` when unavailable or malformed.
 *
 * @param {string | null | undefined} token
 * @returns {number | null}
 */
export function decodeJwtExpMs(token) {
  if (typeof token !== 'string') {
    return null;
  }
  const parts = token.trim().split('.');
  if (parts.length !== 3 || !parts[1]) {
    return null;
  }
  try {
    const normalized = parts[1].replace(/-/g, '+').replace(/_/g, '/');
    const padLen = (4 - (normalized.length % 4)) % 4;
    const padded = `${normalized}${'='.repeat(padLen)}`;
    const jsonText = atob(padded);
    const parsed = JSON.parse(jsonText);
    if (typeof parsed.exp === 'number' && Number.isFinite(parsed.exp) && parsed.exp > 0) {
      return parsed.exp * 1000;
    }
  } catch {
    // Ignore non-JWT or opaque tokens.
  }
  return null;
}

/**
 * Checks if a stored access token expiration timestamp is expired or within `skewSeconds`.
 *
 * @param {number | null | undefined} expiresAtMs
 * @param {number} [skewSeconds=30]
 * @param {number} [nowMs=Date.now()]
 */
export function isTokenExpired(expiresAtMs, skewSeconds = 30, nowMs = Date.now()) {
  if (expiresAtMs === null || expiresAtMs === undefined) {
    return false;
  }
  if (!Number.isFinite(expiresAtMs) || expiresAtMs <= 0) {
    return true;
  }
  return nowMs + skewSeconds * 1000 >= expiresAtMs;
}

/**
 * Ranks and picks the best non-empty X `auth_token` cookie across `x.com` and `twitter.com`.
 *
 * @param {Array<{ name?: string; value?: string; domain?: string; expirationDate?: number; httpOnly?: boolean; secure?: boolean }>} cookies
 */
export function selectBestXCookie(cookies) {
  if (!Array.isArray(cookies) || cookies.length === 0) {
    return null;
  }

  const candidates = cookies.filter(
    (c) =>
      c &&
      typeof c.value === 'string' &&
      c.value.trim().length > 0 &&
      (c.name === undefined || c.name === 'auth_token'),
  );
  if (candidates.length === 0) {
    return null;
  }

  const scoreDomain = (domain) => {
    const d = (domain ?? '').toLowerCase();
    if (d === '.x.com' || d === 'x.com') return 4;
    if (d.endsWith('.x.com')) return 3;
    if (d === '.twitter.com' || d === 'twitter.com') return 2;
    if (d.endsWith('.twitter.com')) return 1;
    return 0;
  };

  const sorted = [...candidates].sort((a, b) => {
    const domainDiff = scoreDomain(b.domain) - scoreDomain(a.domain);
    if (domainDiff !== 0) {
      return domainDiff;
    }
    const expA = typeof a.expirationDate === 'number' ? a.expirationDate : Number.MAX_SAFE_INTEGER;
    const expB = typeof b.expirationDate === 'number' ? b.expirationDate : Number.MAX_SAFE_INTEGER;
    return expB - expA;
  });

  return sorted[0];
}

async function parseErrorResponse(response) {
  let code = `HTTP_${response.status}`;
  let message = response.statusText || `HTTP ${response.status}`;
  try {
    const text = await response.text();
    if (text) {
      try {
        const parsed = JSON.parse(text);
        if (parsed && typeof parsed.code === 'string') {
          code = parsed.code;
        }
        if (parsed && typeof parsed.message === 'string') {
          message = parsed.message;
        } else if (parsed && typeof parsed.error_description === 'string') {
          message = parsed.error_description;
        } else if (parsed && typeof parsed.error === 'string') {
          message = parsed.error;
        }
      } catch {
        message = text.trim() || message;
      }
    }
  } catch {
    // Keep status fallback.
  }
  return new ApiHttpError(message, response.status, code);
}

/**
 * Refreshes an OIDC public-client session using `refresh_token`.
 * Preserves existing `refreshToken` if the IdP omits a rotated token.
 *
 * @param {{ baseUrl: string; session: { accessToken?: string | null; refreshToken?: string | null; expiresAt?: number | null }; fetchImpl?: typeof fetch }} params
 */
export async function refreshOidcSession({ baseUrl, session, fetchImpl }) {
  const fetcher = fetchImpl ?? globalThis.fetch;
  if (!fetcher) {
    throw new Error('Fetch implementation is unavailable');
  }
  const refreshToken = typeof session?.refreshToken === 'string' ? session.refreshToken.trim() : '';
  if (!refreshToken) {
    throw new ApiHttpError('No refresh token available to renew session', 401, 'UNAUTHORIZED');
  }

  const { baseUrl: normalizedBase } = normalizeServerUrl(baseUrl);
  const authCfgResp = await fetcher(`${normalizedBase}${API_ROUTES.AUTH_CONFIG}`, {
    method: 'GET',
    headers: { accept: 'application/json' },
  });
  if (!authCfgResp.ok) {
    throw await parseErrorResponse(authCfgResp);
  }
  const authCfg = await authCfgResp.json();
  if (authCfg.authMode !== 'oidc' || !authCfg.issuerUrl || !authCfg.clientId) {
    throw new ApiHttpError('Server is not configured for OIDC refresh', 400, 'OIDC_UNCONFIGURED');
  }

  const normalizedIssuer = String(authCfg.issuerUrl).trim().replace(/\/+$/, '');
  const discoveryResp = await fetcher(`${normalizedIssuer}/.well-known/openid-configuration`, {
    method: 'GET',
    headers: { accept: 'application/json' },
  });
  if (!discoveryResp.ok) {
    throw await parseErrorResponse(discoveryResp);
  }
  const discovery = await discoveryResp.json();
  const tokenEndpoint = typeof discovery.token_endpoint === 'string' ? discovery.token_endpoint : '';
  if (!tokenEndpoint) {
    throw new Error('OIDC discovery missing token_endpoint');
  }

  const body = new URLSearchParams();
  body.set('grant_type', 'refresh_token');
  body.set('client_id', String(authCfg.clientId));
  body.set('refresh_token', refreshToken);

  const tokenResp = await fetcher(tokenEndpoint, {
    method: 'POST',
    headers: {
      'content-type': 'application/x-www-form-urlencoded',
      accept: 'application/json',
    },
    body: body.toString(),
  });
  if (!tokenResp.ok) {
    throw await parseErrorResponse(tokenResp);
  }
  const payload = await tokenResp.json();
  const nextAccessToken = typeof payload.access_token === 'string' ? payload.access_token.trim() : '';
  if (!nextAccessToken) {
    throw new Error('Token refresh response missing access_token');
  }
  const nextRefreshToken =
    typeof payload.refresh_token === 'string' && payload.refresh_token.trim()
      ? payload.refresh_token.trim()
      : refreshToken;
  const expiresAt =
    typeof payload.expires_in === 'number' && Number.isFinite(payload.expires_in) && payload.expires_in > 0
      ? Date.now() + payload.expires_in * 1000
      : decodeJwtExpMs(nextAccessToken);

  return {
    accessToken: nextAccessToken,
    refreshToken: nextRefreshToken,
    expiresAt,
  };
}

/**
 * Verifies `/api/v1/me` admin role and fetches `/api/v1/admin/platform`.
 *
 * @param {{ baseUrl: string; accessToken?: string | null; fetchImpl?: typeof fetch }} params
 */
export async function fetchAdminPlatformStatus({ baseUrl, accessToken, fetchImpl }) {
  const fetcher = fetchImpl ?? globalThis.fetch;
  const { baseUrl: normalizedBase } = normalizeServerUrl(baseUrl);
  const headers = { accept: 'application/json' };
  const token = typeof accessToken === 'string' ? accessToken.trim() : '';
  if (token) {
    headers.authorization = `Bearer ${token}`;
  }

  const meResp = await fetcher(`${normalizedBase}${API_ROUTES.ME}`, { method: 'GET', headers });
  if (!meResp.ok) {
    throw await parseErrorResponse(meResp);
  }
  const mePayload = await meResp.json();
  const user = mePayload?.user ?? null;
  const role = typeof user?.role === 'string' ? user.role.trim().toLowerCase() : '';
  if (role !== 'admin') {
    throw new ApiHttpError(
      'Current account does not have administrator role',
      403,
      'FORBIDDEN_NOT_ADMIN',
    );
  }

  const platformResp = await fetcher(`${normalizedBase}${API_ROUTES.ADMIN_PLATFORM}`, {
    method: 'GET',
    headers,
  });
  if (!platformResp.ok) {
    throw await parseErrorResponse(platformResp);
  }
  const settings = await platformResp.json();
  return { user, settings };
}

/**
 * Pushes the X `auth_token` cookie to `PUT /api/v1/admin/platform` to encrypt in DB and hot-reload RSSHub.
 *
 * @param {{ baseUrl: string; accessToken?: string | null; authToken: string; fetchImpl?: typeof fetch }} params
 */
export async function pushPlatformXAuthToken({ baseUrl, accessToken, authToken, fetchImpl }) {
  const fetcher = fetchImpl ?? globalThis.fetch;
  const { baseUrl: normalizedBase } = normalizeServerUrl(baseUrl);
  const trimmedCookie = typeof authToken === 'string' ? authToken.trim() : '';
  if (!trimmedCookie) {
    throw new Error('X auth_token must not be empty');
  }

  const headers = {
    'content-type': 'application/json',
    accept: 'application/json',
  };
  const token = typeof accessToken === 'string' ? accessToken.trim() : '';
  if (token) {
    headers.authorization = `Bearer ${token}`;
  }

  const resp = await fetcher(`${normalizedBase}${API_ROUTES.ADMIN_PLATFORM}`, {
    method: 'PUT',
    headers,
    body: JSON.stringify({ xAuthToken: trimmedCookie }),
  });
  if (!resp.ok) {
    throw await parseErrorResponse(resp);
  }
  return resp.json();
}

/**
 * Triggers `POST /api/v1/admin/platform/x/test` to verify end-to-end X collection through RSSHub.
 *
 * @param {{ baseUrl: string; accessToken?: string | null; handle?: string; fetchImpl?: typeof fetch }} params
 */
export async function testPlatformXCollection({ baseUrl, accessToken, handle, fetchImpl }) {
  const fetcher = fetchImpl ?? globalThis.fetch;
  const { baseUrl: normalizedBase } = normalizeServerUrl(baseUrl);
  const cleanHandle = (typeof handle === 'string' ? handle.trim() : '').replace(/^@+/, '');

  const headers = {
    'content-type': 'application/json',
    accept: 'application/json',
  };
  const token = typeof accessToken === 'string' ? accessToken.trim() : '';
  if (token) {
    headers.authorization = `Bearer ${token}`;
  }

  const resp = await fetcher(`${normalizedBase}${API_ROUTES.ADMIN_PLATFORM_X_TEST}`, {
    method: 'POST',
    headers,
    body: JSON.stringify(cleanHandle ? { handle: cleanHandle } : {}),
  });
  if (!resp.ok) {
    throw await parseErrorResponse(resp);
  }
  return resp.json();
}

/**
 * Runs an authenticated server call with automatic pre-expiry refresh and single 401 retry.
 *
 * @template T
 * @param {{ baseUrl: string; session: { accessToken?: string | null; refreshToken?: string | null; expiresAt?: number | null }; operation: (accessToken: string | null) => Promise<T>; fetchImpl?: typeof fetch }} params
 * @returns {Promise<{ result: T; session: { accessToken: string | null; refreshToken: string | null; expiresAt: number | null }; refreshed: boolean }>}
 */
export async function executeWithTokenRefresh({ baseUrl, session, operation, fetchImpl }) {
  let workingSession = {
    accessToken: session?.accessToken?.trim() || null,
    refreshToken: session?.refreshToken?.trim() || null,
    expiresAt: typeof session?.expiresAt === 'number' ? session.expiresAt : null,
  };
  let refreshed = false;

  if (
    workingSession.refreshToken &&
    (!workingSession.accessToken || isTokenExpired(workingSession.expiresAt))
  ) {
    workingSession = await refreshOidcSession({ baseUrl, session: workingSession, fetchImpl });
    refreshed = true;
  }

  try {
    const result = await operation(workingSession.accessToken);
    return { result, session: workingSession, refreshed };
  } catch (err) {
    if (err instanceof ApiHttpError && err.status === 401 && workingSession.refreshToken) {
      workingSession = await refreshOidcSession({ baseUrl, session: workingSession, fetchImpl });
      refreshed = true;
      const result = await operation(workingSession.accessToken);
      return { result, session: workingSession, refreshed };
    }
    throw err;
  }
}
