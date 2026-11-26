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
  type OidcDiscoveryEndpoints,
  buildAuthorizeUrl,
  clearAuthSession,
  clearPkceTransaction,
  computeCodeChallengeS256,
  discoverOidcEndpoints,
  exchangeCodeForToken,
  generateCodeVerifier,
  generateOidcState,
  isPermanentOidcRefreshError,
  isTokenExpired,
  loadAuthSession,
  loadPkceTransaction,
  refreshOidcToken,
  saveAuthSession,
  savePkceTransaction,
} from './oidc';

export interface AuthContextValue {
  authConfig: AuthConfigResponse | null;
  user: UserRecord | null;
  onboarding: OnboardingFlags | null;
  platformXReady: boolean;
  accessToken: string | null;
  isAuthenticated: boolean;
  isLoading: boolean;
  error: string | null;
  serverBaseUrl: string;
  api: ApiClient;
  login: () => Promise<void>;
  logout: () => void;
  handleCallback: (search: string, redirectUriOverride?: string) => Promise<string>;
  refreshAccessToken: () => Promise<string | null>;
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
    if (!stored.refreshToken && isTokenExpired(stored.expiresAt)) {
      clearAuthSession();
      return null;
    }
    return stored;
  }, []);

  const initialValidAccessToken =
    initialSession && !isTokenExpired(initialSession.expiresAt) ? initialSession.accessToken : null;

  const initialServerBaseUrl = useMemo(() => activeBridge.getServerBaseUrl(), [activeBridge]);

  const [serverBaseUrl, setServerBaseUrl] = useState<string>(initialServerBaseUrl);
  const [accessToken, setAccessToken] = useState<string | null>(initialValidAccessToken);
  const [tokenExpiresAt, setTokenExpiresAt] = useState<number | null>(
    initialSession?.expiresAt ?? null,
  );
  const [authConfig, setAuthConfig] = useState<AuthConfigResponse | null>(null);
  const [user, setUser] = useState<UserRecord | null>(null);
  const [onboarding, setOnboarding] = useState<OnboardingFlags | null>(null);
  const [platformXReady, setPlatformXReady] = useState<boolean>(false);
  const [isAuthenticated, setIsAuthenticated] = useState<boolean>(false);
  const [isLoading, setIsLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);
  const [initGeneration, setInitGeneration] = useState<number>(0);

  const serverBaseUrlRef = useRef<string>(serverBaseUrl);
  serverBaseUrlRef.current = serverBaseUrl;

  const accessTokenRef = useRef<string | null>(accessToken);
  accessTokenRef.current = accessToken;

  const refreshTokenRef = useRef<string | null>(initialSession?.refreshToken ?? null);
  const tokenExpiresAtRef = useRef<number | null>(tokenExpiresAt);
  tokenExpiresAtRef.current = tokenExpiresAt;

  const authConfigRef = useRef<AuthConfigResponse | null>(authConfig);
  authConfigRef.current = authConfig;

  const oidcEndpointsCacheRef = useRef<{
    issuerUrl: string;
    endpoints: OidcDiscoveryEndpoints;
  } | null>(null);
  const apiRef = useRef<ApiClient | null>(null);
  const inFlightRefreshRef = useRef<Promise<string | null> | null>(null);
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
    refreshTokenRef.current = null;
    tokenExpiresAtRef.current = null;
    setAccessToken(null);
    setTokenExpiresAt(null);
    setUser(null);
    setOnboarding(null);
    setPlatformXReady(false);
    setIsAuthenticated(false);
    syncDesktopSession(serverBaseUrlRef.current, null, authConfigRef.current?.authMode ?? null);
  }, [syncDesktopSession]);

  const refreshAccessToken = useCallback(async (): Promise<string | null> => {
    if (inFlightRefreshRef.current) {
      return inFlightRefreshRef.current;
    }

    const promise = (async (): Promise<string | null> => {
      const stored = loadAuthSession();
      const activeRefreshToken = refreshTokenRef.current ?? stored?.refreshToken ?? null;
      if (!activeRefreshToken) {
        return null;
      }

      let config = authConfigRef.current;
      if (!config && apiRef.current) {
        config = await apiRef.current.getAuthConfig();
        authConfigRef.current = config;
        setAuthConfig(config);
      }

      if (!config || config.authMode !== AuthMode.OIDC || !config.issuerUrl || !config.clientId) {
        return null;
      }

      let endpoints: OidcDiscoveryEndpoints;
      if (oidcEndpointsCacheRef.current?.issuerUrl === config.issuerUrl) {
        endpoints = oidcEndpointsCacheRef.current.endpoints;
      } else {
        endpoints = await discoverOidcEndpoints(config.issuerUrl, effectiveOidcFetch);
        oidcEndpointsCacheRef.current = { issuerUrl: config.issuerUrl, endpoints };
      }

      const tokenRes = await refreshOidcToken(
        {
          tokenEndpoint: endpoints.tokenEndpoint,
          clientId: config.clientId,
          refreshToken: activeRefreshToken,
        },
        effectiveOidcFetch,
      );

      const saved = saveAuthSession({
        accessToken: tokenRes.accessToken,
        idToken: tokenRes.idToken ?? stored?.idToken ?? null,
        refreshToken: tokenRes.refreshToken ?? activeRefreshToken,
        expiresIn: tokenRes.expiresIn,
      });

      accessTokenRef.current = saved.accessToken;
      refreshTokenRef.current = saved.refreshToken;
      tokenExpiresAtRef.current = saved.expiresAt;
      setAccessToken(saved.accessToken);
      setTokenExpiresAt(saved.expiresAt);
      syncDesktopSession(serverBaseUrlRef.current, saved.accessToken, config.authMode);
      return saved.accessToken;
    })()
      .catch((err: unknown) => {
        if (isPermanentOidcRefreshError(err)) {
          handleUnauthorized();
        }
        return null;
      })
      .finally(() => {
        if (inFlightRefreshRef.current === promise) {
          inFlightRefreshRef.current = null;
        }
      });

    inFlightRefreshRef.current = promise;
    return promise;
  }, [effectiveOidcFetch, handleUnauthorized, syncDesktopSession]);

  const api = useMemo(
    () =>
      createApiClient({
        getBaseUrl: () => serverBaseUrlRef.current,
        getAccessToken: () => accessTokenRef.current,
        refreshAccessToken,
        onUnauthorized: handleUnauthorized,
        fetchImpl,
      }),
    [fetchImpl, handleUnauthorized, refreshAccessToken],
  );
  apiRef.current = api;

  const refreshMe = useCallback(async () => {
    const me = await api.getMe();
    setUser(me.user);
    setOnboarding(me.onboarding);
    setPlatformXReady(Boolean(me.platformXReady));
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
          setPlatformXReady(Boolean(me.platformXReady));
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
        if (!currentSession) {
          clearAuthSession();
          accessTokenRef.current = null;
          refreshTokenRef.current = null;
          tokenExpiresAtRef.current = null;
          setAccessToken(null);
          setTokenExpiresAt(null);
          setUser(null);
          setOnboarding(null);
          setIsAuthenticated(false);
          syncDesktopSession(serverBaseUrlRef.current, null, config.authMode);
          setIsLoading(false);
          return;
        }

        refreshTokenRef.current = currentSession.refreshToken;
        tokenExpiresAtRef.current = currentSession.expiresAt;
        setTokenExpiresAt(currentSession.expiresAt);

        let effectiveAccessToken: string | null = currentSession.accessToken;
        if (isTokenExpired(currentSession.expiresAt)) {
          if (currentSession.refreshToken) {
            effectiveAccessToken = await refreshAccessToken();
            if (cancelled) {
              return;
            }
          } else {
            clearAuthSession();
            effectiveAccessToken = null;
          }
        }

        if (!effectiveAccessToken) {
          accessTokenRef.current = null;
          setAccessToken(null);
          setUser(null);
          setOnboarding(null);
          setIsAuthenticated(false);
          syncDesktopSession(serverBaseUrlRef.current, null, config.authMode);
          setIsLoading(false);
          return;
        }

        accessTokenRef.current = effectiveAccessToken;
        setAccessToken(effectiveAccessToken);
        const me = await api.getMe();
        if (cancelled) {
          return;
        }
        setUser(me.user);
        setOnboarding(me.onboarding);
        setPlatformXReady(Boolean(me.platformXReady));
        setIsAuthenticated(true);
        syncDesktopSession(serverBaseUrlRef.current, accessTokenRef.current, config.authMode);
      } catch (err) {
        if (cancelled) {
          return;
        }
        if (err instanceof ApiError && err.status === 401) {
          setIsAuthenticated(false);
          setError(null);
        } else if (
          activeBridge.runtime === PlatformRuntime.DESKTOP &&
          !serverBaseUrlRef.current.trim()
        ) {
          authConfigRef.current = null;
          setAuthConfig(null);
          setIsAuthenticated(false);
          setError(null);
          syncDesktopSession('', accessTokenRef.current, null);
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
  }, [activeBridge, api, initGeneration, refreshAccessToken, serverBaseUrl, syncDesktopSession]);

  useEffect(() => {
    if (
      !isAuthenticated ||
      authConfig?.authMode !== AuthMode.OIDC ||
      tokenExpiresAt === null ||
      !refreshTokenRef.current
    ) {
      return undefined;
    }

    const targetMs = tokenExpiresAt - DEFAULTS.OIDC_REFRESH_LEAD_SECONDS * 1000;
    const delayMs = Math.min(
      DEFAULTS.OIDC_REFRESH_MAX_DELAY_MS,
      Math.max(DEFAULTS.OIDC_REFRESH_MIN_DELAY_MS, targetMs - Date.now()),
    );

    const timer = setTimeout(() => {
      void refreshAccessToken();
    }, delayMs);

    const checkAndRefreshIfDue = () => {
      if (typeof document !== 'undefined' && document.hidden) {
        return;
      }
      if (
        tokenExpiresAtRef.current !== null &&
        refreshTokenRef.current &&
        isTokenExpired(tokenExpiresAtRef.current, DEFAULTS.OIDC_REFRESH_LEAD_SECONDS)
      ) {
        void refreshAccessToken();
      }
    };

    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', checkAndRefreshIfDue);
    }
    if (typeof window !== 'undefined') {
      window.addEventListener('focus', checkAndRefreshIfDue);
    }

    return () => {
      clearTimeout(timer);
      if (typeof document !== 'undefined') {
        document.removeEventListener('visibilitychange', checkAndRefreshIfDue);
      }
      if (typeof window !== 'undefined') {
        window.removeEventListener('focus', checkAndRefreshIfDue);
      }
    };
  }, [authConfig?.authMode, isAuthenticated, refreshAccessToken, tokenExpiresAt]);

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
        oidcEndpointsCacheRef.current = { issuerUrl: config.issuerUrl, endpoints };
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
          refreshToken: tokenRes.refreshToken ?? null,
          expiresIn: tokenRes.expiresIn,
        });

        accessTokenRef.current = saved.accessToken;
        refreshTokenRef.current = saved.refreshToken;
        tokenExpiresAtRef.current = saved.expiresAt;
        setAccessToken(saved.accessToken);
        setTokenExpiresAt(saved.expiresAt);

        const me = await api.getMe();
        setUser(me.user);
        setOnboarding(me.onboarding);
        setPlatformXReady(Boolean(me.platformXReady));
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
      let config = authConfigRef.current;
      if (!config) {
        if (activeBridge.runtime === PlatformRuntime.DESKTOP && !serverBaseUrlRef.current.trim()) {
          try {
            config = await api.getAuthConfig();
          } catch {
            throw new Error('Please configure and apply the Server Base URL first.');
          }
        } else {
          config = await api.getAuthConfig();
        }
      }
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
      oidcEndpointsCacheRef.current = { issuerUrl: config.issuerUrl, endpoints };
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
    refreshTokenRef.current = null;
    tokenExpiresAtRef.current = null;
    setAccessToken(null);
    setTokenExpiresAt(null);
    setUser(null);
    setOnboarding(null);
    setPlatformXReady(false);
    setIsAuthenticated(false);
    setError(null);
    syncDesktopSession(serverBaseUrlRef.current, null, authConfigRef.current?.authMode ?? null);
  }, [syncDesktopSession]);

  const updateServerBaseUrl = useCallback(
    (url: string) => {
      activeBridge.setServerBaseUrl(url);
      const normalized = activeBridge.getServerBaseUrl();
      serverBaseUrlRef.current = normalized;
      oidcEndpointsCacheRef.current = null;
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
      platformXReady,
      accessToken,
      isAuthenticated,
      isLoading,
      error,
      serverBaseUrl,
      api,
      login,
      logout,
      handleCallback,
      refreshAccessToken,
      refreshMe,
      updateServerBaseUrl,
    }),
    [
      authConfig,
      user,
      onboarding,
      platformXReady,
      accessToken,
      isAuthenticated,
      isLoading,
      error,
      serverBaseUrl,
      api,
      login,
      logout,
      handleCallback,
      refreshAccessToken,
      refreshMe,
      updateServerBaseUrl,
    ],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useOptionalAuth(): AuthContextValue | null {
  return useContext(AuthContext);
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) {
    throw new Error('useAuth must be used within an AuthProvider');
  }
  return ctx;
}
