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
import { APP_ROUTES, STORAGE_KEYS } from '../constants/routes';
import { WebPlatformBridge, isAllowedExternalUrl } from '../platform/bridge';
import { AuthMode, McpAccessMode } from '../types/enums';

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function AuthStatusProbe() {
  const { authConfig, isAuthenticated, isLoading, user, error, login } = useAuth();
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
});
