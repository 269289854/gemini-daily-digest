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
  loadPkceTransaction,
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
  const { authConfig, isAuthenticated, isLoading, user, error, login, logout, updateServerBaseUrl } =
    useAuth();
  return (
    <div>
      <span data-testid="loading">{String(isLoading)}</span>
      <span data-testid="authed">{String(isAuthenticated)}</span>
      <span data-testid="mode">{authConfig?.authMode ?? 'none'}</span>
      <span data-testid="email">{user?.email ?? 'none'}</span>
      <span data-testid="error">{error ?? 'none'}</span>
      <button type="button" onClick={() => void login().catch(() => {})}>
        Trigger Login
      </button>
      <button type="button" onClick={() => logout()}>
        Trigger Logout
      </button>
      <button type="button" onClick={() => updateServerBaseUrl('https://updated.example.com/')}>
        Update Server URL
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
});
