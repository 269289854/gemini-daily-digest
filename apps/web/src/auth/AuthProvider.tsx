import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { ApiError, type ApiClient, createApiClient } from '../api/client';
import { APP_ROUTES, DEFAULTS } from '../constants/routes';
import {
  type PlatformBridge,
  PlatformRuntime,
  createPlatformBridge,
  extractOidcCallbackSearch,
} from '../platform/bridge';
import type { AuthConfigResponse, OnboardingFlags, UserRecord } from '../types/api';
import { AuthMode } from '../types/enums';
import {
  buildAuthorizeUrl,
  clearAuthSession,
  clearPkceTransaction,
  computeCodeChallengeS256,
  discoverOidcEndpoints,
  exchangeCodeForToken,
  generateCodeVerifier,
  generateOidcState,
  isTokenExpired,
  loadAuthSession,
  loadPkceTransaction,
  saveAuthSession,
  savePkceTransaction,
} from './oidc';

export interface AuthContextValue {
  authConfig: AuthConfigResponse | null;
  user: UserRecord | null;
  onboarding: OnboardingFlags | null;
  accessToken: string | null;
  isAuthenticated: boolean;
  isLoading: boolean;
  error: string | null;
  serverBaseUrl: string;
  api: ApiClient;
  login: () => Promise<void>;
  logout: () => void;
  handleCallback: (search: string, redirectUriOverride?: string) => Promise<string>;
  refreshMe: () => Promise<void>;
  updateServerBaseUrl: (url: string) => void;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export interface AuthProviderProps {
  bridge?: PlatformBridge;
  fetchImpl?: typeof fetch;
  children: React.ReactNode;
}

function getWebRedirectUri(): string {
  if (typeof window === 'undefined' || !window.location?.origin) {
    return APP_ROUTES.AUTH_CALLBACK;
  }
  return `${window.location.origin}${APP_ROUTES.AUTH_CALLBACK}`;
}

function getCurrentRedirectTarget(): string | undefined {
  if (typeof window === 'undefined' || !window.location) {
    return undefined;
  }
  const pathname = window.location.pathname;
  if (!pathname || pathname === '/' || pathname === APP_ROUTES.AUTH_CALLBACK) {
    return APP_ROUTES.DAILY;
  }
  return `${pathname}${window.location.search ?? ''}`;
}

function sanitizePostLoginRedirect(path: string | null): string {
  if (!path || !path.startsWith('/') || path.startsWith('//') || path.startsWith(APP_ROUTES.AUTH_CALLBACK)) {
    return APP_ROUTES.DAILY;
  }
  return path;
}

export function AuthProvider({ bridge, fetchImpl, children }: AuthProviderProps) {
  const activeBridge = useMemo(() => bridge ?? createPlatformBridge(), [bridge]);
  const effectiveOidcFetch = useMemo(
    () => fetchImpl ?? activeBridge.oidcFetch ?? globalThis.fetch,
    [activeBridge, fetchImpl],
  );

  const initialSession = useMemo(() => {
    const stored = loadAuthSession();
    if (!stored) {
      return null;
    }
    if (isTokenExpired(stored.expiresAt)) {
      clearAuthSession();
      return null;
    }
    return stored;
  }, []);

  const initialServerBaseUrl = useMemo(() => activeBridge.getServerBaseUrl(), [activeBridge]);

  const [serverBaseUrl, setServerBaseUrl] = useState<string>(initialServerBaseUrl);
  const [accessToken, setAccessToken] = useState<string | null>(initialSession?.accessToken ?? null);
  const [authConfig, setAuthConfig] = useState<AuthConfigResponse | null>(null);
  const [user, setUser] = useState<UserRecord | null>(null);
  const [onboarding, setOnboarding] = useState<OnboardingFlags | null>(null);
  const [isAuthenticated, setIsAuthenticated] = useState<boolean>(false);
  const [isLoading, setIsLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);
  const [initGeneration, setInitGeneration] = useState<number>(0);

  const serverBaseUrlRef = useRef<string>(serverBaseUrl);
  serverBaseUrlRef.current = serverBaseUrl;

  const accessTokenRef = useRef<string | null>(accessToken);
  accessTokenRef.current = accessToken;

  const authConfigRef = useRef<AuthConfigResponse | null>(authConfig);
  authConfigRef.current = authConfig;

  const inFlightCallbackRef = useRef<{ search: string; promise: Promise<string> } | null>(null);

  const syncDesktopSession = useCallback(
    (nextServerBaseUrl: string, nextAccessToken: string | null, nextAuthMode: AuthMode | null) => {
      if (!activeBridge.syncRuntimeSession) {
        return;
      }
      void activeBridge
        .syncRuntimeSession({
          serverBaseUrl: nextServerBaseUrl,
          accessToken: nextAccessToken,
          authMode: nextAuthMode,
        })
        .catch(() => {
          // Ignore desktop runtime sync failures in non-desktop or transient startup states.
        });
    },
    [activeBridge],
  );

  const handleUnauthorized = useCallback(() => {
    clearAuthSession();
    accessTokenRef.current = null;
    setAccessToken(null);
    setUser(null);
    setOnboarding(null);
    setIsAuthenticated(false);
    syncDesktopSession(serverBaseUrlRef.current, null, authConfigRef.current?.authMode ?? null);
  }, [syncDesktopSession]);

  const api = useMemo(
    () =>
      createApiClient({
        getBaseUrl: () => serverBaseUrlRef.current,
        getAccessToken: () => accessTokenRef.current,
        onUnauthorized: handleUnauthorized,
        fetchImpl,
      }),
    [fetchImpl, handleUnauthorized],
  );

  const refreshMe = useCallback(async () => {
    const me = await api.getMe();
    setUser(me.user);
    setOnboarding(me.onboarding);
    setIsAuthenticated(true);
    setError(null);
  }, [api]);

  useEffect(() => {
    let cancelled = false;

    async function bootstrap() {
      setIsLoading(true);
      setError(null);
      try {
        const config = await api.getAuthConfig();
        if (cancelled) {
          return;
        }
        authConfigRef.current = config;
        setAuthConfig(config);

        if (config.authMode === AuthMode.DEV_BYPASS) {
          const me = await api.getMe();
          if (cancelled) {
            return;
          }
          setUser(me.user);
          setOnboarding(me.onboarding);
          setIsAuthenticated(true);
          syncDesktopSession(serverBaseUrlRef.current, null, config.authMode);
          setIsLoading(false);
          return;
        }

        const isCallbackRoute =
          typeof window !== 'undefined' && window.location?.pathname === APP_ROUTES.AUTH_CALLBACK;
        if (isCallbackRoute) {
          setIsLoading(false);
          return;
        }

        const currentSession = loadAuthSession();
        if (!currentSession || isTokenExpired(currentSession.expiresAt)) {
          clearAuthSession();
          accessTokenRef.current = null;
          setAccessToken(null);
          setUser(null);
          setOnboarding(null);
          setIsAuthenticated(false);
          syncDesktopSession(serverBaseUrlRef.current, null, config.authMode);
          setIsLoading(false);
          return;
        }

        accessTokenRef.current = currentSession.accessToken;
        setAccessToken(currentSession.accessToken);
        const me = await api.getMe();
        if (cancelled) {
          return;
        }
        setUser(me.user);
        setOnboarding(me.onboarding);
        setIsAuthenticated(true);
        syncDesktopSession(serverBaseUrlRef.current, currentSession.accessToken, config.authMode);
      } catch (err) {
        if (cancelled) {
          return;
        }
        if (err instanceof ApiError && err.status === 401) {
          setIsAuthenticated(false);
          setError(null);
        } else {
          setIsAuthenticated(false);
          setError(err instanceof Error ? err.message : 'Failed to initialize authentication');
          syncDesktopSession(
            serverBaseUrlRef.current,
            accessTokenRef.current,
            authConfigRef.current?.authMode ?? null,
          );
        }
      } finally {
        if (!cancelled) {
          setIsLoading(false);
        }
      }
    }

    void bootstrap();

    return () => {
      cancelled = true;
    };
  }, [api, initGeneration, serverBaseUrl, syncDesktopSession]);

  const handleCallback = useCallback(
    async (search: string, redirectUriOverride?: string): Promise<string> => {
      const flightKey = `${search}::${redirectUriOverride ?? ''}`;
      if (inFlightCallbackRef.current && inFlightCallbackRef.current.search === flightKey) {
        return inFlightCallbackRef.current.promise;
      }

      const promise = (async (): Promise<string> => {
        setError(null);
        const params = new URLSearchParams(search.startsWith('?') ? search.slice(1) : search);
        const oauthError = params.get('error');
        const oauthErrorDesc = params.get('error_description');
        if (oauthError) {
          clearPkceTransaction();
          const msg = oauthErrorDesc ? `${oauthError}: ${oauthErrorDesc}` : oauthError;
          setError(msg);
          throw new Error(msg);
        }

        const code = params.get('code')?.trim() ?? '';
        const returnedState = params.get('state')?.trim() ?? '';
        const {
          verifier,
          state: storedState,
          postLoginRedirect,
          redirectUri: storedRedirectUri,
        } = loadPkceTransaction();

        // Clear PKCE verifier, state, and redirect URI immediately after reading/validation.
        clearPkceTransaction();

        if (!code || !returnedState) {
          const msg = 'Missing authorization code or state parameter in OIDC callback';
          setError(msg);
          throw new Error(msg);
        }

        if (!storedState || returnedState !== storedState || !verifier) {
          const msg = 'OIDC state mismatch or expired login session. Please sign in again.';
          setError(msg);
          throw new Error(msg);
        }

        const config = authConfigRef.current ?? (await api.getAuthConfig());
        authConfigRef.current = config;
        setAuthConfig(config);

        if (!config.issuerUrl || !config.clientId) {
          const msg = 'OIDC configuration is incomplete (missing issuerUrl or clientId)';
          setError(msg);
          throw new Error(msg);
        }

        const effectiveRedirectUri =
          redirectUriOverride ?? storedRedirectUri ?? getWebRedirectUri();
        const endpoints = await discoverOidcEndpoints(config.issuerUrl, effectiveOidcFetch);
        const tokenRes = await exchangeCodeForToken(
          {
            tokenEndpoint: endpoints.tokenEndpoint,
            clientId: config.clientId,
            code,
            redirectUri: effectiveRedirectUri,
            codeVerifier: verifier,
          },
          effectiveOidcFetch,
        );

        const saved = saveAuthSession({
          accessToken: tokenRes.accessToken,
          idToken: tokenRes.idToken,
          expiresIn: tokenRes.expiresIn,
        });

        accessTokenRef.current = saved.accessToken;
        setAccessToken(saved.accessToken);

        const me = await api.getMe();
        setUser(me.user);
        setOnboarding(me.onboarding);
        setIsAuthenticated(true);
        setError(null);
        syncDesktopSession(serverBaseUrlRef.current, saved.accessToken, config.authMode);

        return sanitizePostLoginRedirect(postLoginRedirect);
      })();

      inFlightCallbackRef.current = { search: flightKey, promise };
      try {
        return await promise;
      } finally {
        if (inFlightCallbackRef.current?.promise === promise) {
          inFlightCallbackRef.current = null;
        }
      }
    },
    [api, effectiveOidcFetch, syncDesktopSession],
  );

  const login = useCallback(async (): Promise<void> => {
    setError(null);
    try {
      const config = authConfigRef.current ?? (await api.getAuthConfig());
      authConfigRef.current = config;
      setAuthConfig(config);

      if (config.authMode === AuthMode.DEV_BYPASS) {
        await refreshMe();
        syncDesktopSession(serverBaseUrlRef.current, accessTokenRef.current, config.authMode);
        return;
      }

      if (!config.issuerUrl || !config.clientId) {
        throw new Error('OIDC configuration is incomplete (missing issuerUrl or clientId)');
      }

      const endpoints = await discoverOidcEndpoints(config.issuerUrl, effectiveOidcFetch);
      const verifier = generateCodeVerifier();
      const state = generateOidcState();
      const codeChallenge = await computeCodeChallengeS256(verifier);

      const useDesktopLoopback =
        activeBridge.runtime === PlatformRuntime.DESKTOP &&
        Boolean(activeBridge.openOidcSystemLogin);
      const redirectUri =
        useDesktopLoopback && activeBridge.prepareOidcLoopback
          ? await activeBridge.prepareOidcLoopback()
          : getWebRedirectUri();

      savePkceTransaction(verifier, state, getCurrentRedirectTarget(), redirectUri);

      const authorizeUrl = buildAuthorizeUrl({
        authorizationEndpoint: endpoints.authorizationEndpoint,
        clientId: config.clientId,
        redirectUri,
        scopes: config.scopes?.length ? config.scopes : DEFAULTS.DEFAULT_OIDC_SCOPES,
        state,
        codeChallenge,
      });

      if (useDesktopLoopback && activeBridge.openOidcSystemLogin) {
        const callbackPayload = await activeBridge.openOidcSystemLogin(authorizeUrl);
        const callbackSearch = extractOidcCallbackSearch(callbackPayload);
        if (!callbackSearch) {
          throw new Error('OIDC login was cancelled or returned an empty authorization callback');
        }
        await handleCallback(callbackSearch, redirectUri);
        return;
      }

      if (typeof window !== 'undefined' && window.location) {
        window.location.assign(authorizeUrl);
      }
    } catch (err) {
      clearPkceTransaction();
      const message = err instanceof Error ? err.message : 'Unable to start OIDC login';
      setError(message);
      throw err;
    }
  }, [activeBridge, api, effectiveOidcFetch, handleCallback, refreshMe, syncDesktopSession]);

  const logout = useCallback(() => {
    clearAuthSession();
    clearPkceTransaction();
    accessTokenRef.current = null;
    setAccessToken(null);
    setUser(null);
    setOnboarding(null);
    setIsAuthenticated(false);
    setError(null);
    syncDesktopSession(serverBaseUrlRef.current, null, authConfigRef.current?.authMode ?? null);
  }, [syncDesktopSession]);

  const updateServerBaseUrl = useCallback(
    (url: string) => {
      activeBridge.setServerBaseUrl(url);
      const normalized = activeBridge.getServerBaseUrl();
      serverBaseUrlRef.current = normalized;
      setServerBaseUrl(normalized);
      setError(null);
      syncDesktopSession(normalized, accessTokenRef.current, authConfigRef.current?.authMode ?? null);
      setInitGeneration((prev) => prev + 1);
    },
    [activeBridge, syncDesktopSession],
  );

  const value = useMemo<AuthContextValue>(
    () => ({
      authConfig,
      user,
      onboarding,
      accessToken,
      isAuthenticated,
      isLoading,
      error,
      serverBaseUrl,
      api,
      login,
      logout,
      handleCallback,
      refreshMe,
      updateServerBaseUrl,
    }),
    [
      authConfig,
      user,
      onboarding,
      accessToken,
      isAuthenticated,
      isLoading,
      error,
      serverBaseUrl,
      api,
      login,
      logout,
      handleCallback,
      refreshMe,
      updateServerBaseUrl,
    ],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) {
    throw new Error('useAuth must be used within an AuthProvider');
  }
  return ctx;
}
