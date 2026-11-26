import { useState } from 'react';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AuthCallbackPage } from '../auth/AuthCallbackPage';
import { AuthProvider, useAuth } from '../auth/AuthProvider';
import {
  buildAuthorizeUrl,
  clearAuthSession,
  clearPkceTransaction,
  computeCodeChallengeS256,
  generateCodeVerifier,
  generateOidcState,
  loadAuthSession,
  loadPkceTransaction,
  saveAuthSession,
  savePkceTransaction,
} from '../auth/oidc';
import { APP_ROUTES, DEFAULTS, STORAGE_KEYS } from '../constants/routes';
import {
  type PlatformBridge,
  PlatformRuntime,
  WebPlatformBridge,
  isAllowedExternalUrl,
} from '../platform/bridge';
import { AuthMode, McpAccessMode } from '../types/enums';

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function AuthStatusProbe() {
  const {
    api,
    authConfig,
    accessToken,
    isAuthenticated,
    isLoading,
    user,
    error,
    login,
    logout,
    updateServerBaseUrl,
  } = useAuth();
  const [concurrentState, setConcurrentState] = useState('idle');

  const triggerConcurrentCalls = async () => {
    try {
      const [sources, channels] = await Promise.all([api.listSources(), api.listChannels()]);
      setConcurrentState(`ok:${sources.length}:${channels.length}`);
    } catch (err) {
      setConcurrentState(`err:${err instanceof Error ? err.message : String(err)}`);
    }
  };

  return (
    <div>
      <span data-testid="loading">{String(isLoading)}</span>
      <span data-testid="authed">{String(isAuthenticated)}</span>
      <span data-testid="mode">{authConfig?.authMode ?? 'none'}</span>
      <span data-testid="email">{user?.email ?? 'none'}</span>
      <span data-testid="token">{accessToken ?? 'none'}</span>
      <span data-testid="error">{error ?? 'none'}</span>
      <span data-testid="concurrent-state">{concurrentState}</span>
      <button type="button" onClick={() => void login().catch(() => {})}>
        Trigger Login
      </button>
      <button type="button" onClick={() => logout()}>
        Trigger Logout
      </button>
      <button type="button" onClick={() => updateServerBaseUrl('https://updated.example.com/')}>
        Update Server URL
      </button>
      <button type="button" onClick={() => void triggerConcurrentCalls()}>
        Trigger Concurrent Calls
      </button>
    </div>
  );
}

describe('PlatformBridge, OIDC PKCE helpers, AuthCallbackPage, and AuthProvider', () => {
  beforeEach(() => {
    window.localStorage.clear();
    window.sessionStorage.clear();
    clearAuthSession();
    clearPkceTransaction();
    window.history.replaceState(null, '', '/');
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('validates external URLs and enforces http/https only in WebPlatformBridge', async () => {
    expect(isAllowedExternalUrl('https://ai.google.dev/docs')).toBe(true);
    expect(isAllowedExternalUrl('http://127.0.0.1:8080/health')).toBe(true);
    expect(isAllowedExternalUrl('javascript:alert(1)')).toBe(false);
    expect(isAllowedExternalUrl('data:text/html,<script>alert(1)</script>')).toBe(false);
    expect(isAllowedExternalUrl('vbscript:msgbox(1)')).toBe(false);
    expect(isAllowedExternalUrl('file:///etc/passwd')).toBe(false);
    expect(isAllowedExternalUrl('/relative/path')).toBe(false);
    expect(isAllowedExternalUrl('')).toBe(false);

    const openSpy = vi.spyOn(window, 'open').mockImplementation(() => null);
    const bridge = new WebPlatformBridge();

    await bridge.openExternalUrl('https://ai.google.dev/gemini-api');
    expect(openSpy).toHaveBeenCalledWith(
      'https://ai.google.dev/gemini-api',
      '_blank',
      'noopener,noreferrer',
    );

    openSpy.mockClear();
    await expect(bridge.openExternalUrl('javascript:alert(1)')).rejects.toThrow(/Blocked unsafe/i);
    await expect(bridge.openExternalUrl('data:text/plain,bad')).rejects.toThrow(/Blocked unsafe/i);
    await expect(bridge.openExternalUrl('vbscript:evil()')).rejects.toThrow(/Blocked unsafe/i);
    expect(openSpy).not.toHaveBeenCalled();
  });

  it('persists server base URL and MCP config in WebPlatformBridge', () => {
    const bridge = new WebPlatformBridge();
    expect(bridge.getServerBaseUrl()).toBe('');

    bridge.setServerBaseUrl('https://digest.example.com///');
    expect(bridge.getServerBaseUrl()).toBe('https://digest.example.com');
    expect(window.localStorage.getItem(STORAGE_KEYS.SERVER_BASE_URL)).toBe(
      'https://digest.example.com',
    );

    bridge.setServerBaseUrl('');
    expect(bridge.getServerBaseUrl()).toBe('');

    expect(bridge.getMcpConfig()).toEqual({ mode: McpAccessMode.READ_ONLY, port: 8765 });
    bridge.setMcpConfig(McpAccessMode.READ_WRITE, 9123);
    expect(bridge.getMcpConfig()).toEqual({ mode: McpAccessMode.READ_WRITE, port: 9123 });
  });

  it('generates valid PKCE S256 verifier, challenge, state, and authorize URL', async () => {
    const verifier = generateCodeVerifier();
    const state = generateOidcState();
    const challenge = await computeCodeChallengeS256(verifier);

    expect(verifier.length).toBeGreaterThanOrEqual(40);
    expect(state.length).toBeGreaterThanOrEqual(24);
    expect(challenge).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(challenge).not.toBe(verifier);

    const authUrl = buildAuthorizeUrl({
      authorizationEndpoint: 'https://auth.example.com/application/o/authorize/',
      clientId: 'client-abc',
      redirectUri: 'https://digest.example.com/auth/callback',
      state,
      codeChallenge: challenge,
    });
    const parsed = new URL(authUrl);
    expect(parsed.searchParams.get('response_type')).toBe('code');
    expect(parsed.searchParams.get('client_id')).toBe('client-abc');
    expect(parsed.searchParams.get('scope')).toContain('offline_access');
    expect(parsed.searchParams.get('state')).toBe(state);
    expect(parsed.searchParams.get('code_challenge')).toBe(challenge);
    expect(parsed.searchParams.get('code_challenge_method')).toBe('S256');
  });

  it('rejects OIDC state mismatch and missing PKCE_VERIFIER in AuthCallbackPage', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockImplementation(async () =>
      jsonResponse({
        authMode: AuthMode.OIDC,
        issuerUrl: 'https://auth.example.com/application/o/digest/',
        clientId: 'client-abc',
        scopes: ['openid', 'profile', 'email'],
      }),
    );

    // Case 1: state mismatch
    savePkceTransaction('verifier-123', 'expected-state', APP_ROUTES.HISTORY);
    const onSuccess = vi.fn();
    const onBackToLogin = vi.fn();

    render(
      <AuthProvider fetchImpl={fetchMock}>
        <AuthCallbackPage
          search="?code=auth-code-1&state=wrong-state"
          onSuccess={onSuccess}
          onBackToLogin={onBackToLogin}
        />
      </AuthProvider>,
    );

    const alert1 = await screen.findByRole('alert');
    expect(alert1.textContent).toMatch(/state mismatch/i);
    expect(onSuccess).not.toHaveBeenCalled();
    expect(loadPkceTransaction().verifier).toBeNull();

    cleanup();

    // Case 2: missing PKCE_VERIFIER
    window.sessionStorage.setItem(STORAGE_KEYS.OIDC_STATE, 'matching-state');
    window.sessionStorage.removeItem(STORAGE_KEYS.PKCE_VERIFIER);
    window.localStorage.removeItem(STORAGE_KEYS.PKCE_VERIFIER);

    render(
      <AuthProvider fetchImpl={fetchMock}>
        <AuthCallbackPage
          search="?code=auth-code-2&state=matching-state"
          onSuccess={onSuccess}
          onBackToLogin={onBackToLogin}
        />
      </AuthProvider>,
    );

    const alert2 = await screen.findByRole('alert');
    expect(alert2.textContent).toMatch(/state mismatch or expired/i);
    screen.getByRole('button', { name: /Back to Sign In/i }).click();
    expect(onBackToLogin).toHaveBeenCalledTimes(1);
  });

  it('completes AuthCallbackPage OIDC exchange when state and verifier match', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/v1/auth/config')) {
        return jsonResponse({
          authMode: AuthMode.OIDC,
          issuerUrl: 'https://auth.example.com/application/o/digest',
          clientId: 'client-abc',
          scopes: ['openid', 'profile', 'email'],
        });
      }
      if (url.endsWith('/.well-known/openid-configuration')) {
        return jsonResponse({
          authorization_endpoint: 'https://auth.example.com/authorize',
          token_endpoint: 'https://auth.example.com/token',
        });
      }
      if (url === 'https://auth.example.com/token') {
        return jsonResponse({
          access_token: 'test-tok-123',
          id_token: 'id-tok-456',
          refresh_token: 'refresh-tok-789',
          expires_in: 3600,
        });
      }
      if (url.endsWith('/api/v1/me')) {
        return jsonResponse({
          user: {
            id: 'u1',
            oidcSub: 'sub-oidc-1',
            email: 'user@example.com',
            displayName: 'Example User',
            role: 'user',
            createdAt: '2026-11-26T00:00:00Z',
            lastLoginAt: null,
          },
          onboarding: {
            hasAiConfig: true,
            hasSources: true,
            hasChannels: true,
            hasSchedule: true,
          },
        });
      }
      return jsonResponse({}, 404);
    });

    savePkceTransaction('verifier-xyz', 'state-xyz', '/sources?sourceId=src-1');
    const onSuccess = vi.fn();

    render(
      <AuthProvider fetchImpl={fetchMock}>
        <AuthCallbackPage
          search="?code=code-xyz&state=state-xyz"
          onSuccess={onSuccess}
          onBackToLogin={vi.fn()}
        />
      </AuthProvider>,
    );

    await waitFor(() => {
      expect(onSuccess).toHaveBeenCalledWith('/sources?sourceId=src-1');
    });
    expect(window.localStorage.getItem(STORAGE_KEYS.ACCESS_TOKEN)).toBe('test-tok-123');
    expect(window.localStorage.getItem(STORAGE_KEYS.REFRESH_TOKEN)).toBe('refresh-tok-789');
  });

  it('auto-authenticates in DEV_BYPASS mode and stays unauthenticated in OIDC mode without token', async () => {
    const devFetch = vi.fn<typeof fetch>().mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/v1/auth/config')) {
        return jsonResponse({
          authMode: AuthMode.DEV_BYPASS,
          issuerUrl: null,
          clientId: null,
          scopes: [],
        });
      }
      if (url.endsWith('/api/v1/me')) {
        return jsonResponse({
          user: {
            id: 'dev-user',
            oidcSub: 'dev-bypass',
            email: 'dev@example.com',
            displayName: 'Dev User',
            role: 'admin',
            createdAt: '2026-11-26T00:00:00Z',
            lastLoginAt: null,
          },
          onboarding: {
            hasAiConfig: false,
            hasSources: false,
            hasChannels: false,
            hasSchedule: false,
          },
        });
      }
      return jsonResponse({}, 404);
    });

    render(
      <AuthProvider fetchImpl={devFetch}>
        <AuthStatusProbe />
      </AuthProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('loading').textContent).toBe('false');
    });
    expect(screen.getByTestId('authed').textContent).toBe('true');
    expect(screen.getByTestId('mode').textContent).toBe(AuthMode.DEV_BYPASS);
    expect(screen.getByTestId('email').textContent).toBe('dev@example.com');

    cleanup();

    const oidcFetch = vi.fn<typeof fetch>().mockImplementation(async () =>
      jsonResponse({
        authMode: AuthMode.OIDC,
        issuerUrl: 'https://auth.example.com/o/digest',
        clientId: 'client-1',
        scopes: ['openid', 'profile'],
      }),
    );

    render(
      <AuthProvider fetchImpl={oidcFetch}>
        <AuthStatusProbe />
      </AuthProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('loading').textContent).toBe('false');
    });
    expect(screen.getByTestId('authed').textContent).toBe('false');
    expect(screen.getByTestId('mode').textContent).toBe(AuthMode.OIDC);
    expect(screen.getByTestId('email').textContent).toBe('none');
  });

  it('maintains Desktop OIDC loopback redirectUri parity across authorize and token exchange and syncs session', async () => {
    const loopbackRedirectUri = 'http://127.0.0.1:49152/callback';
    let capturedAuthRedirectUri: string | null = null;
    let capturedTokenBody = '';
    const syncRuntimeSession = vi.fn().mockResolvedValue(undefined);

    const oidcFetch = vi.fn<typeof fetch>().mockImplementation(async (input, init) => {
      const url = String(input);
      if (url.endsWith('/.well-known/openid-configuration')) {
        return jsonResponse({
          authorization_endpoint: 'https://auth.example.com/authorize',
          token_endpoint: 'https://auth.example.com/token',
        });
      }
      if (url === 'https://auth.example.com/token') {
        capturedTokenBody = typeof init?.body === 'string' ? init.body : '';
        return jsonResponse({
          access_token: 'desktop-access-tok',
          id_token: 'desktop-id-tok',
          expires_in: 3600,
        });
      }
      return jsonResponse({}, 404);
    });

    const apiFetch = vi.fn<typeof fetch>().mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/v1/auth/config')) {
        return jsonResponse({
          authMode: AuthMode.OIDC,
          issuerUrl: 'https://auth.example.com/o/digest',
          clientId: 'desktop-client-1',
          scopes: ['openid', 'profile'],
        });
      }
      if (url.endsWith('/api/v1/me')) {
        return jsonResponse({
          user: {
            id: 'u-desk',
            oidcSub: 'sub-desk',
            email: 'desktop@example.com',
            displayName: 'Desktop User',
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
      return jsonResponse({}, 404);
    });

    // Combined fetcher used when fetchImpl is omitted on AuthProvider? Wait: AuthProvider routes OIDC calls
    // via `fetchImpl ?? activeBridge.oidcFetch ?? globalThis.fetch`. So omit `fetchImpl` and mock globalThis.fetch
    // for API calls while `bridge.oidcFetch` handles OIDC discovery/token exchange!
    vi.spyOn(globalThis, 'fetch').mockImplementation(apiFetch);

    const openOidcSystemLogin = vi.fn(async (authorizeUrl: string) => {
      const parsed = new URL(authorizeUrl);
      capturedAuthRedirectUri = parsed.searchParams.get('redirect_uri');
      const state = parsed.searchParams.get('state') ?? '';
      expect(window.sessionStorage.getItem(STORAGE_KEYS.OIDC_REDIRECT_URI)).toBe(
        loopbackRedirectUri,
      );
      return `http://127.0.0.1:49152/callback?code=loopback-code-1&state=${encodeURIComponent(state)}`;
    });

    const desktopBridge: PlatformBridge = {
      runtime: PlatformRuntime.DESKTOP,
      openExternalUrl: vi.fn().mockResolvedValue(undefined),
      notify: vi.fn().mockResolvedValue(undefined),
      copyToClipboard: vi.fn().mockResolvedValue(undefined),
      getServerBaseUrl: vi.fn().mockReturnValue('https://api.example.com'),
      setServerBaseUrl: vi.fn().mockImplementation((u: string) => {
        desktopBridge.getServerBaseUrl = vi.fn().mockReturnValue(u.replace(/\/+$/, ''));
      }),
      getMcpConfig: vi.fn().mockReturnValue({
        mode: McpAccessMode.READ_ONLY,
        port: DEFAULTS.DEFAULT_MCP_HTTP_PORT,
      }),
      setMcpConfig: vi.fn(),
      prepareOidcLoopback: vi.fn().mockResolvedValue(loopbackRedirectUri),
      openOidcSystemLogin,
      oidcFetch,
      syncRuntimeSession,
    };

    render(
      <AuthProvider bridge={desktopBridge}>
        <AuthStatusProbe />
      </AuthProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('loading').textContent).toBe('false');
    });
    expect(syncRuntimeSession).toHaveBeenCalledWith({
      serverBaseUrl: 'https://api.example.com',
      accessToken: null,
      authMode: AuthMode.OIDC,
    });

    screen.getByRole('button', { name: /Trigger Login/i }).click();

    await waitFor(() => {
      expect(screen.getByTestId('authed').textContent).toBe('true');
    });

    expect(desktopBridge.prepareOidcLoopback).toHaveBeenCalledTimes(1);
    expect(capturedAuthRedirectUri).toBe(loopbackRedirectUri);
    const tokenParams = new URLSearchParams(capturedTokenBody);
    expect(tokenParams.get('redirect_uri')).toBe(loopbackRedirectUri);
    // 1) discovery in login(), 2) discovery in handleCallback(), 3) token exchange in handleCallback()
    expect(oidcFetch).toHaveBeenCalledTimes(3);
    expect(screen.getByTestId('email').textContent).toBe('desktop@example.com');
    expect(loadPkceTransaction().redirectUri).toBeNull();
    expect(syncRuntimeSession).toHaveBeenLastCalledWith({
      serverBaseUrl: 'https://api.example.com',
      accessToken: 'desktop-access-tok',
      authMode: AuthMode.OIDC,
    });

    // Logout & updateServerBaseUrl also sync runtime session
    screen.getByRole('button', { name: /Trigger Logout/i }).click();
    expect(syncRuntimeSession).toHaveBeenLastCalledWith({
      serverBaseUrl: 'https://api.example.com',
      accessToken: null,
      authMode: AuthMode.OIDC,
    });

    screen.getByRole('button', { name: /Update Server URL/i }).click();
    await waitFor(() => {
      expect(syncRuntimeSession).toHaveBeenCalledWith({
        serverBaseUrl: 'https://updated.example.com',
        accessToken: null,
        authMode: AuthMode.OIDC,
      });
    });
  });

  it('surfaces an explicit error when Desktop openOidcSystemLogin returns empty callback search', async () => {
    const desktopBridge: PlatformBridge = {
      runtime: PlatformRuntime.DESKTOP,
      openExternalUrl: vi.fn().mockResolvedValue(undefined),
      notify: vi.fn().mockResolvedValue(undefined),
      copyToClipboard: vi.fn().mockResolvedValue(undefined),
      getServerBaseUrl: vi.fn().mockReturnValue('https://api.example.com'),
      setServerBaseUrl: vi.fn(),
      getMcpConfig: vi.fn().mockReturnValue({
        mode: McpAccessMode.READ_ONLY,
        port: DEFAULTS.DEFAULT_MCP_HTTP_PORT,
      }),
      setMcpConfig: vi.fn(),
      prepareOidcLoopback: vi.fn().mockResolvedValue('http://127.0.0.1:50001/callback'),
      openOidcSystemLogin: vi.fn().mockResolvedValue(''),
    };

    const fetchMock = vi.fn<typeof fetch>().mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/v1/auth/config')) {
        return jsonResponse({
          authMode: AuthMode.OIDC,
          issuerUrl: 'https://auth.example.com/o/digest',
          clientId: 'desktop-client-1',
          scopes: ['openid'],
        });
      }
      if (url.endsWith('/.well-known/openid-configuration')) {
        return jsonResponse({
          authorization_endpoint: 'https://auth.example.com/authorize',
          token_endpoint: 'https://auth.example.com/token',
        });
      }
      return jsonResponse({}, 404);
    });

    render(
      <AuthProvider bridge={desktopBridge} fetchImpl={fetchMock}>
        <AuthStatusProbe />
      </AuthProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('loading').textContent).toBe('false');
    });

    screen.getByRole('button', { name: /Trigger Login/i }).click();

    await waitFor(() => {
      expect(screen.getByTestId('error').textContent).toMatch(/cancelled|empty/i);
    });
    expect(screen.getByTestId('authed').textContent).toBe('false');
    expect(loadPkceTransaction().verifier).toBeNull();
    expect(loadPkceTransaction().redirectUri).toBeNull();
  });

  it('completes callback cleanly even when parent re-renders with new onSuccess reference mid-flight', async () => {
    let resolveTokenExchange: ((value: Response) => void) | null = null;
    const tokenPromise = new Promise<Response>((resolve) => {
      resolveTokenExchange = resolve;
    });

    const fetchMock = vi.fn<typeof fetch>().mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/v1/auth/config')) {
        return jsonResponse({
          authMode: AuthMode.OIDC,
          issuerUrl: 'https://auth.example.com/application/o/digest',
          clientId: 'client-abc',
          scopes: ['openid', 'profile', 'email'],
        });
      }
      if (url.endsWith('/.well-known/openid-configuration')) {
        return jsonResponse({
          authorization_endpoint: 'https://auth.example.com/authorize',
          token_endpoint: 'https://auth.example.com/token',
        });
      }
      if (url === 'https://auth.example.com/token') {
        return tokenPromise;
      }
      if (url.endsWith('/api/v1/me')) {
        return jsonResponse({
          user: {
            id: 'u1',
            oidcSub: 'sub-oidc-1',
            email: 'user@example.com',
            displayName: 'Example User',
            role: 'user',
            createdAt: '2026-11-26T00:00:00Z',
            lastLoginAt: null,
          },
          onboarding: {
            hasAiConfig: true,
            hasSources: true,
            hasChannels: true,
            hasSchedule: true,
          },
        });
      }
      return jsonResponse({}, 404);
    });

    savePkceTransaction('verifier-rerender', 'state-rerender', APP_ROUTES.DAILY);
    const firstOnSuccess = vi.fn();
    const secondOnSuccess = vi.fn();

    const { rerender } = render(
      <AuthProvider fetchImpl={fetchMock}>
        <AuthCallbackPage
          search="?code=code-rr&state=state-rerender"
          onSuccess={firstOnSuccess}
          onBackToLogin={vi.fn()}
        />
      </AuthProvider>,
    );

    // Wait until token exchange has started, then simulate parent re-render with a new onSuccess callback.
    await waitFor(() => {
      expect(resolveTokenExchange).not.toBeNull();
    });

    rerender(
      <AuthProvider fetchImpl={fetchMock}>
        <AuthCallbackPage
          search="?code=code-rr&state=state-rerender"
          onSuccess={secondOnSuccess}
          onBackToLogin={vi.fn()}
        />
      </AuthProvider>,
    );

    resolveTokenExchange!(
      jsonResponse({
        access_token: 'tok-after-rerender',
        id_token: 'id-after-rerender',
        expires_in: 3600,
      }),
    );

    await waitFor(() => {
      expect(secondOnSuccess).toHaveBeenCalledWith(APP_ROUTES.DAILY);
    });
    expect(firstOnSuccess).not.toHaveBeenCalled();
  });

  it('surfaces error and stops spinner when callback fails after parent re-render', async () => {
    let resolveMe: ((value: Response) => void) | null = null;
    const mePromise = new Promise<Response>((resolve) => {
      resolveMe = resolve;
    });

    const fetchMock = vi.fn<typeof fetch>().mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/v1/auth/config')) {
        return jsonResponse({
          authMode: AuthMode.OIDC,
          issuerUrl: 'https://auth.example.com/application/o/digest',
          clientId: 'client-abc',
          scopes: ['openid', 'profile', 'email'],
        });
      }
      if (url.endsWith('/.well-known/openid-configuration')) {
        return jsonResponse({
          authorization_endpoint: 'https://auth.example.com/authorize',
          token_endpoint: 'https://auth.example.com/token',
        });
      }
      if (url === 'https://auth.example.com/token') {
        return jsonResponse({
          access_token: 'jwe.five.part.token.sample',
          expires_in: 3600,
        });
      }
      if (url.endsWith('/api/v1/me')) {
        return mePromise;
      }
      return jsonResponse({}, 404);
    });

    savePkceTransaction('verifier-err', 'state-err', APP_ROUTES.DAILY);

    const { rerender } = render(
      <AuthProvider fetchImpl={fetchMock}>
        <AuthCallbackPage
          search="?code=code-err&state=state-err"
          onSuccess={vi.fn()}
          onBackToLogin={vi.fn()}
        />
      </AuthProvider>,
    );

    await waitFor(() => {
      expect(resolveMe).not.toBeNull();
    });

    rerender(
      <AuthProvider fetchImpl={fetchMock}>
        <AuthCallbackPage
          search="?code=code-err&state=state-err"
          onSuccess={vi.fn()}
          onBackToLogin={vi.fn()}
        />
      </AuthProvider>,
    );

    resolveMe!(
      jsonResponse(
        {
          code: 'UNAUTHORIZED',
          message: 'encrypted JWT (JWE) is not supported',
        },
        401,
      ),
    );

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toMatch(/JWE/i);
    expect(screen.queryByText(/Completing Sign-In/i)).toBeNull();
  });

  it('silently refreshes expired access_token on bootstrap when refresh_token is stored and rotates tokens', async () => {
    window.localStorage.setItem(STORAGE_KEYS.ACCESS_TOKEN, 'expired-boot-tok');
    window.localStorage.setItem(STORAGE_KEYS.REFRESH_TOKEN, 'boot-refresh-1');
    window.localStorage.setItem(STORAGE_KEYS.TOKEN_EXPIRES_AT, String(Date.now() - 60_000));

    let refreshGrantSeen = '';
    const fetchMock = vi.fn<typeof fetch>().mockImplementation(async (input, init) => {
      const url = String(input);
      if (url.endsWith('/api/v1/auth/config')) {
        return jsonResponse({
          authMode: AuthMode.OIDC,
          issuerUrl: 'https://auth.example.com/o/digest',
          clientId: 'client-1',
          scopes: ['openid', 'profile', 'email', 'offline_access'],
        });
      }
      if (url.endsWith('/.well-known/openid-configuration')) {
        return jsonResponse({
          authorization_endpoint: 'https://auth.example.com/authorize',
          token_endpoint: 'https://auth.example.com/token',
        });
      }
      if (url === 'https://auth.example.com/token') {
        const params = new URLSearchParams(typeof init?.body === 'string' ? init.body : '');
        refreshGrantSeen = `${params.get('grant_type')}:${params.get('refresh_token')}`;
        return jsonResponse({
          access_token: 'boot-fresh-access',
          refresh_token: 'boot-rotated-refresh-2',
          expires_in: 3600,
        });
      }
      if (url.endsWith('/api/v1/me')) {
        const headers = init?.headers as Headers;
        if (headers.get('authorization') === 'Bearer boot-fresh-access') {
          return jsonResponse({
            user: {
              id: 'u-boot',
              oidcSub: 'sub-boot',
              email: 'boot@example.com',
              displayName: 'Boot User',
              role: 'admin',
              createdAt: '2026-11-26T00:00:00Z',
              lastLoginAt: null,
            },
            onboarding: {
              hasAiConfig: true,
              hasSources: true,
              hasChannels: true,
              hasSchedule: true,
            },
          });
        }
        return jsonResponse({ code: 'UNAUTHORIZED', message: 'Expired' }, 401);
      }
      return jsonResponse({}, 404);
    });

    render(
      <AuthProvider fetchImpl={fetchMock}>
        <AuthStatusProbe />
      </AuthProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('loading').textContent).toBe('false');
    });
    expect(screen.getByTestId('authed').textContent).toBe('true');
    expect(screen.getByTestId('email').textContent).toBe('boot@example.com');
    expect(screen.getByTestId('token').textContent).toBe('boot-fresh-access');
    expect(refreshGrantSeen).toBe('refresh_token:boot-refresh-1');
    expect(loadAuthSession()).toMatchObject({
      accessToken: 'boot-fresh-access',
      refreshToken: 'boot-rotated-refresh-2',
    });
  });

  it('deduplicates concurrent 401 refreshes into a single /token call and syncs rotated token to desktop bridge', async () => {
    saveAuthSession({
      accessToken: 'stale-tok-1',
      refreshToken: 'shared-refresh-1',
      expiresIn: 3600,
    });

    const syncRuntimeSession = vi.fn().mockResolvedValue(undefined);
    const desktopBridge: PlatformBridge = {
      runtime: PlatformRuntime.DESKTOP,
      openExternalUrl: vi.fn().mockResolvedValue(undefined),
      notify: vi.fn().mockResolvedValue(undefined),
      copyToClipboard: vi.fn().mockResolvedValue(undefined),
      getServerBaseUrl: vi.fn().mockReturnValue('https://api.example.com'),
      setServerBaseUrl: vi.fn(),
      getMcpConfig: vi.fn().mockReturnValue({ mode: McpAccessMode.READ_ONLY, port: 8765 }),
      setMcpConfig: vi.fn(),
      syncRuntimeSession,
    };

    let tokenRefreshCalls = 0;
    const fetchMock = vi.fn<typeof fetch>().mockImplementation(async (input, init) => {
      const url = String(input);
      if (url.endsWith('/api/v1/auth/config')) {
        return jsonResponse({
          authMode: AuthMode.OIDC,
          issuerUrl: 'https://auth.example.com/o/digest',
          clientId: 'client-1',
          scopes: ['openid', 'profile', 'email', 'offline_access'],
        });
      }
      if (url.endsWith('/.well-known/openid-configuration')) {
        return jsonResponse({
          authorization_endpoint: 'https://auth.example.com/authorize',
          token_endpoint: 'https://auth.example.com/token',
        });
      }
      if (url === 'https://auth.example.com/token') {
        tokenRefreshCalls += 1;
        await new Promise((resolve) => setTimeout(resolve, 15));
        return jsonResponse({
          access_token: 'rotated-tok-2',
          expires_in: 3600,
        });
      }
      const headers = init?.headers as Headers | undefined;
      const auth = headers?.get('authorization');
      if (url.endsWith('/api/v1/me')) {
        return jsonResponse({
          user: {
            id: 'u-1',
            oidcSub: 'sub-1',
            email: 'user@example.com',
            displayName: 'User',
            role: 'admin',
            createdAt: '2026-11-26T00:00:00Z',
            lastLoginAt: null,
          },
          onboarding: { hasAiConfig: true, hasSources: true, hasChannels: true, hasSchedule: true },
        });
      }
      if (url.endsWith('/api/v1/settings/sources') || url.endsWith('/api/v1/settings/channels')) {
        if (auth === 'Bearer stale-tok-1') {
          return jsonResponse({ code: 'UNAUTHORIZED', message: 'Expired' }, 401);
        }
        if (auth === 'Bearer rotated-tok-2') {
          return jsonResponse([]);
        }
      }
      return jsonResponse({}, 404);
    });

    render(
      <AuthProvider bridge={desktopBridge} fetchImpl={fetchMock}>
        <AuthStatusProbe />
      </AuthProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('authed').textContent).toBe('true');
    });

    screen.getByRole('button', { name: /Trigger Concurrent Calls/i }).click();

    await waitFor(() => {
      expect(screen.getByTestId('concurrent-state').textContent).toBe('ok:0:0');
    });
    expect(tokenRefreshCalls).toBe(1);
    expect(screen.getByTestId('token').textContent).toBe('rotated-tok-2');
    // Preserved existing refresh_token when IdP omits rotated refresh_token on refresh
    expect(loadAuthSession()?.refreshToken).toBe('shared-refresh-1');
    expect(syncRuntimeSession).toHaveBeenLastCalledWith({
      serverBaseUrl: 'https://api.example.com',
      accessToken: 'rotated-tok-2',
      authMode: AuthMode.OIDC,
    });
  });

  it('clears stored session and falls back to Sign-In on permanent invalid_grant refresh failure', async () => {
    window.localStorage.setItem(STORAGE_KEYS.ACCESS_TOKEN, 'expired-boot-tok');
    window.localStorage.setItem(STORAGE_KEYS.REFRESH_TOKEN, 'revoked-refresh');
    window.localStorage.setItem(STORAGE_KEYS.TOKEN_EXPIRES_AT, String(Date.now() - 10_000));

    const fetchMock = vi.fn<typeof fetch>().mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/v1/auth/config')) {
        return jsonResponse({
          authMode: AuthMode.OIDC,
          issuerUrl: 'https://auth.example.com/o/digest',
          clientId: 'client-1',
          scopes: ['openid', 'profile', 'email', 'offline_access'],
        });
      }
      if (url.endsWith('/.well-known/openid-configuration')) {
        return jsonResponse({
          authorization_endpoint: 'https://auth.example.com/authorize',
          token_endpoint: 'https://auth.example.com/token',
        });
      }
      if (url === 'https://auth.example.com/token') {
        return jsonResponse(
          { error: 'invalid_grant', error_description: 'Refresh token revoked' },
          400,
        );
      }
      return jsonResponse({}, 404);
    });

    render(
      <AuthProvider fetchImpl={fetchMock}>
        <AuthStatusProbe />
      </AuthProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('loading').textContent).toBe('false');
    });
    expect(screen.getByTestId('authed').textContent).toBe('false');
    expect(window.localStorage.getItem(STORAGE_KEYS.ACCESS_TOKEN)).toBeNull();
    expect(window.localStorage.getItem(STORAGE_KEYS.REFRESH_TOKEN)).toBeNull();
  });

  it('proactively refreshes access_token via scheduled timer before expiry', async () => {
    vi.useFakeTimers();
    try {
      saveAuthSession({
        accessToken: 'short-lived-tok',
        refreshToken: 'proactive-refresh-1',
        expiresIn: 90,
      });

      const fetchMock = vi.fn<typeof fetch>().mockImplementation(async (input) => {
        const url = String(input);
        if (url.endsWith('/api/v1/auth/config')) {
          return jsonResponse({
            authMode: AuthMode.OIDC,
            issuerUrl: 'https://auth.example.com/o/digest',
            clientId: 'client-1',
            scopes: ['openid', 'profile', 'email', 'offline_access'],
          });
        }
        if (url.endsWith('/.well-known/openid-configuration')) {
          return jsonResponse({
            authorization_endpoint: 'https://auth.example.com/authorize',
            token_endpoint: 'https://auth.example.com/token',
          });
        }
        if (url === 'https://auth.example.com/token') {
          return jsonResponse({
            access_token: 'proactive-new-tok',
            refresh_token: 'proactive-refresh-2',
            expires_in: 3600,
          });
        }
        if (url.endsWith('/api/v1/me')) {
          return jsonResponse({
            user: {
              id: 'u-timer',
              oidcSub: 'sub-timer',
              email: 'timer@example.com',
              displayName: 'Timer User',
              role: 'user',
              createdAt: '2026-11-26T00:00:00Z',
              lastLoginAt: null,
            },
            onboarding: {
              hasAiConfig: true,
              hasSources: true,
              hasChannels: true,
              hasSchedule: true,
            },
          });
        }
        return jsonResponse({}, 404);
      });

      render(
        <AuthProvider fetchImpl={fetchMock}>
          <AuthStatusProbe />
        </AuthProvider>,
      );

      await vi.waitFor(() => {
        expect(screen.getByTestId('authed').textContent).toBe('true');
      });
      expect(screen.getByTestId('token').textContent).toBe('short-lived-tok');

      // 90s expiry - 60s lead = 30s delay
      await vi.advanceTimersByTimeAsync(31_000);
      await vi.waitFor(() => {
        expect(screen.getByTestId('token').textContent).toBe('proactive-new-tok');
      });
      expect(loadAuthSession()?.refreshToken).toBe('proactive-refresh-2');
    } finally {
      vi.useRealTimers();
    }
  });

  it('suppresses HTML fallback JSON parse error in Desktop runtime when Server Base URL is empty', async () => {
    const fetchMock = vi.fn<typeof fetch>(async () =>
      new Response('<!DOCTYPE html><html><body>SPA</body></html>', {
        status: 200,
        headers: { 'content-type': 'text/html' },
      }),
    );
    const desktopBridge: PlatformBridge = {
      runtime: PlatformRuntime.DESKTOP,
      openExternalUrl: vi.fn().mockResolvedValue(undefined),
      notify: vi.fn().mockResolvedValue(undefined),
      copyToClipboard: vi.fn().mockResolvedValue(undefined),
      getServerBaseUrl: vi.fn().mockReturnValue(''),
      setServerBaseUrl: vi.fn(),
      getMcpConfig: vi.fn().mockReturnValue({ mode: McpAccessMode.READ_ONLY, port: 8765 }),
      setMcpConfig: vi.fn(),
      syncRuntimeSession: vi.fn().mockResolvedValue(undefined),
    };

    render(
      <AuthProvider bridge={desktopBridge} fetchImpl={fetchMock}>
        <AuthStatusProbe />
      </AuthProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('loading').textContent).toBe('false');
    });
    expect(screen.getByTestId('authed').textContent).toBe('false');
    expect(screen.getByTestId('error').textContent).toBe('none');

    screen.getByRole('button', { name: /Trigger Login/i }).click();
    await waitFor(() => {
      expect(screen.getByTestId('error').textContent).toMatch(/Server Base URL/i);
    });
  });
});
