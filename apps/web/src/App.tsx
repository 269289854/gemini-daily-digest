import { type FormEvent, useCallback, useEffect, useMemo, useState } from 'react';
import { AlertCircle, Globe, Loader2, LogIn, Monitor, Sparkles } from 'lucide-react';
import { normalizeBaseUrl } from './api/client';
import { AuthCallbackPage } from './auth/AuthCallbackPage';
import { AuthProvider, useAuth } from './auth/AuthProvider';
import { Layout } from './components/Layout';
import { APP_INFO, APP_ROUTES, DEFAULTS } from './constants/routes';
import { useSse } from './hooks/useSse';
import { DailyDigestPage } from './pages/DailyDigestPage';
import { HistorySearchPage } from './pages/HistorySearchPage';
import { RunsStatsPage } from './pages/RunsStatsPage';
import { SETTINGS_TABS, SettingsPage, type SettingsTab } from './pages/SettingsPage';
import { SourcesExplorerPage } from './pages/SourcesExplorerPage';
import { type PlatformBridge, PlatformRuntime, createPlatformBridge } from './platform/bridge';
import type { DigestReadyEvent } from './types/api';
import { AuthMode } from './types/enums';

export type AppRoute = (typeof APP_ROUTES)[keyof typeof APP_ROUTES];

export interface RouteQueryParams {
  date?: string;
  sourceId?: string;
  q?: string;
  bookmarked?: boolean;
  tab?: SettingsTab;
}

export interface RouteState {
  route: AppRoute;
  params: RouteQueryParams;
}

export interface AppProps {
  bridge?: PlatformBridge;
  fetchImpl?: typeof fetch;
}

const VALID_APP_ROUTES: ReadonlySet<string> = new Set(Object.values(APP_ROUTES));
const VALID_SETTINGS_TABS: ReadonlySet<string> = new Set(Object.values(SETTINGS_TABS));

export function normalizeAppRoute(rawRoute: string): AppRoute {
  const trimmed = rawRoute.trim();
  if (!trimmed || trimmed === '/') {
    return APP_ROUTES.DAILY;
  }
  const withSlash = trimmed.startsWith('/') ? trimmed : `/${trimmed}`;
  const normalized = withSlash.length > 1 ? withSlash.replace(/\/+$/, '') : withSlash;
  if (VALID_APP_ROUTES.has(normalized)) {
    return normalized as AppRoute;
  }
  return APP_ROUTES.DAILY;
}

function parseQueryParams(search: string | URLSearchParams): RouteQueryParams {
  const sp = typeof search === 'string' ? new URLSearchParams(search) : search;
  const date = sp.get('date')?.trim() || undefined;
  const sourceId = sp.get('sourceId')?.trim() || undefined;
  const q = sp.get('q')?.trim() || undefined;
  const rawBookmarked = sp.get('bookmarked')?.trim().toLowerCase();
  const bookmarked =
    rawBookmarked === 'true' || rawBookmarked === '1'
      ? true
      : rawBookmarked === 'false' || rawBookmarked === '0'
        ? false
        : undefined;
  const rawTab = sp.get('tab')?.trim();
  const tab = rawTab && VALID_SETTINGS_TABS.has(rawTab) ? (rawTab as SettingsTab) : undefined;
  return { date, sourceId, q, bookmarked, tab };
}

function readBrowserRouteState(): RouteState {
  if (typeof window === 'undefined' || !window.location) {
    return { route: APP_ROUTES.DAILY, params: {} };
  }
  return {
    route: normalizeAppRoute(window.location.pathname),
    params: parseQueryParams(window.location.search),
  };
}

function parseTargetLocation(
  target: string,
  extraParams?: Record<string, string>,
): { route: AppRoute; params: RouteQueryParams; url: string } {
  const parsed = new URL(target.startsWith('/') ? target : `/${target}`, 'http://localhost');
  const route = normalizeAppRoute(parsed.pathname);
  const search = new URLSearchParams(parsed.search);

  if (extraParams) {
    for (const [key, value] of Object.entries(extraParams)) {
      if (value.trim() !== '') {
        search.set(key, value.trim());
      } else {
        search.delete(key);
      }
    }
  }

  const qs = search.toString();
  const url = qs ? `${route}?${qs}` : route;
  return {
    route,
    params: parseQueryParams(search),
    url,
  };
}

interface AuthenticatedShellProps {
  bridge: PlatformBridge;
  fetchImpl?: typeof fetch;
  routeState: RouteState;
  onNavigate: (route: string, params?: Record<string, string>) => void;
}

function AuthenticatedShell({
  bridge,
  fetchImpl,
  routeState,
  onNavigate,
}: AuthenticatedShellProps) {
  const { api, accessToken, serverBaseUrl, updateServerBaseUrl } = useAuth();
  const [latestDigestReady, setLatestDigestReady] = useState<DigestReadyEvent | null>(null);

  const sse = useSse({
    enabled: true,
    baseUrl: serverBaseUrl,
    accessToken,
    fetchImpl,
    onDigestReady: (event) => {
      setLatestDigestReady(event);
    },
    onAlertTriggered: (event) => {
      if (event.count > 0) {
        const firstTitle = event.items[0]?.title ?? 'Urgent watchlist match';
        void bridge.notify(`Urgent Watchlist Alert (${event.count})`, firstTitle);
      }
    },
  });

  const renderPage = () => {
    switch (routeState.route) {
      case APP_ROUTES.SOURCES:
        return (
          <SourcesExplorerPage
            api={api}
            bridge={bridge}
            initialSourceId={routeState.params.sourceId}
          />
        );
      case APP_ROUTES.HISTORY:
        return (
          <HistorySearchPage
            api={api}
            bridge={bridge}
            initialQuery={routeState.params.q}
            initialBookmarked={routeState.params.bookmarked}
          />
        );
      case APP_ROUTES.RUNS:
        return <RunsStatsPage api={api} latestRunProgress={sse.latestRunProgress} />;
      case APP_ROUTES.SETTINGS:
        return (
          <SettingsPage
            api={api}
            bridge={bridge}
            serverBaseUrl={serverBaseUrl}
            onServerBaseUrlChange={updateServerBaseUrl}
            initialTab={routeState.params.tab}
          />
        );
      case APP_ROUTES.DAILY:
      default:
        return (
          <DailyDigestPage
            api={api}
            bridge={bridge}
            latestRunProgress={sse.latestRunProgress}
            latestDigestReady={latestDigestReady}
            initialDate={routeState.params.date}
            onNavigate={onNavigate}
          />
        );
    }
  };

  return (
    <Layout
      currentRoute={routeState.route}
      onNavigate={onNavigate}
      sseStatus={sse.status}
      latestAlert={sse.latestAlert}
      onDismissAlert={sse.clearLatestAlert}
      bridge={bridge}
    >
      {renderPage()}
    </Layout>
  );
}

interface SignInScreenProps {
  bridge: PlatformBridge;
}

function SignInScreen({ bridge }: SignInScreenProps) {
  const { authConfig, error, login, serverBaseUrl, updateServerBaseUrl } = useAuth();
  const [signingIn, setSigningIn] = useState(false);
  const [showServerConfig, setShowServerConfig] = useState(
    () => bridge.runtime === PlatformRuntime.DESKTOP || Boolean(serverBaseUrl),
  );
  const [draftBaseUrl, setDraftBaseUrl] = useState(serverBaseUrl);
  const [urlError, setUrlError] = useState<string | null>(null);
  const [urlSavedMessage, setUrlSavedMessage] = useState<string | null>(null);

  useEffect(() => {
    setDraftBaseUrl(serverBaseUrl);
  }, [serverBaseUrl]);

  const handleLogin = async () => {
    setSigningIn(true);
    try {
      await login();
    } catch {
      // AuthProvider captures and surfaces error state.
    } finally {
      setSigningIn(false);
    }
  };

  const handleApplyServerUrl = (event: FormEvent) => {
    event.preventDefault();
    setUrlError(null);
    setUrlSavedMessage(null);
    try {
      const normalized = normalizeBaseUrl(draftBaseUrl);
      updateServerBaseUrl(normalized);
      setDraftBaseUrl(normalized);
      setUrlSavedMessage(
        normalized
          ? `Server URL set to ${normalized}`
          : 'Server URL reset to same-origin relative mode.',
      );
    } catch (err) {
      setUrlError(err instanceof Error ? err.message : 'Invalid server URL');
    }
  };

  const isDevBypass = authConfig?.authMode === AuthMode.DEV_BYPASS;
  const combinedError = urlError ?? error;

  return (
    <div className="flex min-h-screen items-center justify-center bg-slate-950 px-4 py-12 text-slate-100">
      <div className="w-full max-w-md space-y-6 rounded-2xl border border-slate-800 bg-slate-900/80 p-8 shadow-2xl">
        <div className="flex items-start gap-3">
          <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl bg-gradient-to-br from-emerald-500/25 to-sky-500/20 text-emerald-300 ring-1 ring-emerald-500/30">
            <Sparkles className="h-5 w-5" />
          </div>
          <div className="space-y-1">
            <div className="flex flex-wrap items-center gap-2">
              <h1 className="text-xl font-bold tracking-tight text-white">{APP_INFO.NAME}</h1>
              {bridge.runtime === PlatformRuntime.DESKTOP && (
                <span className="inline-flex items-center gap-1 rounded-md border border-sky-500/30 bg-sky-500/10 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-sky-300">
                  <Monitor className="h-3 w-3" />
                  Desktop
                </span>
              )}
            </div>
            <p className="text-xs text-slate-400">
              Sign in to access your daily AI ecosystem digest, per-source timelines, and alert
              controls.
            </p>
          </div>
        </div>

        {combinedError && (
          <div
            role="alert"
            className="flex items-start gap-2.5 rounded-xl border border-rose-500/40 bg-rose-950/40 p-3.5 text-xs text-rose-200"
          >
            <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-rose-400" />
            <span>{combinedError}</span>
          </div>
        )}

        {urlSavedMessage && (
          <div
            role="status"
            className="rounded-xl border border-emerald-500/40 bg-emerald-950/40 px-3.5 py-2.5 text-xs text-emerald-200"
          >
            {urlSavedMessage}
          </div>
        )}

        <button
          type="button"
          onClick={() => void handleLogin()}
          disabled={signingIn}
          className="inline-flex w-full items-center justify-center gap-2 rounded-xl bg-indigo-600 px-4 py-2.5 text-sm font-semibold text-white transition hover:bg-indigo-500 focus:outline-none focus:ring-2 focus:ring-indigo-400 disabled:cursor-not-allowed disabled:opacity-60"
        >
          {signingIn ? <Loader2 className="h-4 w-4 animate-spin" /> : <LogIn className="h-4 w-4" />}
          {signingIn ? 'Signing in...' : isDevBypass ? 'Continue with Dev Bypass' : 'Sign in with OIDC'}
        </button>

        <div className="border-t border-slate-800 pt-4">
          <button
            type="button"
            onClick={() => setShowServerConfig((prev) => !prev)}
            aria-expanded={showServerConfig}
            className="inline-flex items-center gap-1.5 text-xs font-medium text-slate-400 transition hover:text-slate-200"
          >
            <Globe className="h-3.5 w-3.5" />
            {showServerConfig ? 'Hide Server Base URL' : 'Configure Server Base URL'}
          </button>

          {showServerConfig && (
            <form onSubmit={handleApplyServerUrl} className="mt-3 space-y-2.5">
              <label className="block space-y-1 text-xs">
                <span className="font-medium text-slate-300">Server Base URL</span>
                <input
                  type="text"
                  aria-label="Server Base URL"
                  value={draftBaseUrl}
                  onChange={(e) => setDraftBaseUrl(e.target.value)}
                  placeholder={`Relative same-origin or ${DEFAULTS.DEFAULT_LOCAL_SERVER_URL}`}
                  className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-xs text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
                />
              </label>
              <div className="flex items-center justify-between gap-2">
                <span className="text-[11px] text-slate-500">
                  Leave blank for relative web API paths.
                </span>
                <button
                  type="submit"
                  className="rounded-lg border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs font-medium text-slate-200 transition hover:bg-slate-700"
                >
                  Apply URL
                </button>
              </div>
            </form>
          )}
        </div>
      </div>
    </div>
  );
}

interface AppRouterProps {
  bridge: PlatformBridge;
  fetchImpl?: typeof fetch;
}

function AppRouter({ bridge, fetchImpl }: AppRouterProps) {
  const { authConfig, isLoading, isAuthenticated } = useAuth();
  const [routeState, setRouteState] = useState<RouteState>(() => readBrowserRouteState());

  const navigate = useCallback((targetRoute: string, params?: Record<string, string>) => {
    const resolved = parseTargetLocation(targetRoute, params);
    if (typeof window !== 'undefined' && window.history?.pushState) {
      const currentFull = `${window.location.pathname}${window.location.search}`;
      if (currentFull !== resolved.url) {
        window.history.pushState(null, '', resolved.url);
      }
    }
    setRouteState({ route: resolved.route, params: resolved.params });
  }, []);

  useEffect(() => {
    if (typeof window === 'undefined') {
      return undefined;
    }
    const handlePopState = () => {
      setRouteState(readBrowserRouteState());
    };
    window.addEventListener('popstate', handlePopState);
    return () => {
      window.removeEventListener('popstate', handlePopState);
    };
  }, []);

  useEffect(() => {
    if (!bridge.onDesktopNavigate) {
      return undefined;
    }
    return bridge.onDesktopNavigate((targetRoute, params) => {
      navigate(targetRoute, params);
    });
  }, [bridge, navigate]);

  if (routeState.route === APP_ROUTES.AUTH_CALLBACK) {
    return (
      <AuthCallbackPage
        onSuccess={(redirectPath) => navigate(redirectPath)}
        onBackToLogin={() => navigate(APP_ROUTES.DAILY)}
      />
    );
  }

  if (isLoading && !authConfig) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-slate-950 px-4 text-slate-200">
        <div className="flex items-center gap-3 rounded-xl border border-slate-800 bg-slate-900/80 px-5 py-4 text-sm">
          <Loader2 className="h-5 w-5 animate-spin text-emerald-400" />
          <span>Loading {APP_INFO.NAME}...</span>
        </div>
      </div>
    );
  }

  if (!isAuthenticated) {
    return <SignInScreen bridge={bridge} />;
  }

  return (
    <AuthenticatedShell
      bridge={bridge}
      fetchImpl={fetchImpl}
      routeState={routeState}
      onNavigate={navigate}
    />
  );
}

export function App({ bridge, fetchImpl }: AppProps = {}) {
  const activeBridge = useMemo(() => bridge ?? createPlatformBridge(), [bridge]);

  return (
    <AuthProvider bridge={activeBridge} fetchImpl={fetchImpl}>
      <AppRouter bridge={activeBridge} fetchImpl={fetchImpl} />
    </AuthProvider>
  );
}

export default App;
