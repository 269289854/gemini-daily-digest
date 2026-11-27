import {
  Activity,
  AlertTriangle,
  ArrowRight,
  Bookmark,
  CheckCircle2,
  ExternalLink,
  LogOut,
  Menu,
  Monitor,
  Newspaper,
  Radio,
  Settings,
  Shield,
  Sparkles,
  X,
} from 'lucide-react';
import React, { useState } from 'react';
import { useAuth } from '../auth/AuthProvider';
import { APP_ROUTES } from '../constants/routes';
import type { SseConnectionStatus } from '../hooks/useSse';
import { type TranslationKey, useI18n } from '../i18n';
import { type PlatformBridge, PlatformRuntime, isAllowedExternalUrl } from '../platform/bridge';
import type { AlertTriggeredEvent } from '../types/api';
import { AuthMode } from '../types/enums';
import { LanguageSwitcher } from './LanguageSwitcher';

export interface LayoutProps {
  currentRoute: string;
  onNavigate: (route: string, params?: Record<string, string>) => void;
  sseStatus: SseConnectionStatus;
  latestAlert: AlertTriggeredEvent | null;
  onDismissAlert?: () => void;
  bridge: PlatformBridge;
  children: React.ReactNode;
}

interface NavItem {
  route: string;
  labelKey: TranslationKey;
  icon: React.ComponentType<{ className?: string }>;
}

const NAV_ITEMS: readonly NavItem[] = [
  { route: APP_ROUTES.DAILY, labelKey: 'nav.daily', icon: Newspaper },
  { route: APP_ROUTES.SOURCES, labelKey: 'nav.sources', icon: Radio },
  { route: APP_ROUTES.HISTORY, labelKey: 'nav.history', icon: Bookmark },
  { route: APP_ROUTES.RUNS, labelKey: 'nav.runs', icon: Activity },
  { route: APP_ROUTES.SETTINGS, labelKey: 'nav.settings', icon: Settings },
];

function getSseStatusDisplay(status: SseConnectionStatus): {
  labelKey: TranslationKey;
  dotClass: string;
  pillClass: string;
} {
  switch (status) {
    case 'connected':
      return {
        labelKey: 'sse.live',
        dotClass: 'bg-emerald-400',
        pillClass: 'border-emerald-500/30 bg-emerald-500/10 text-emerald-300',
      };
    case 'connecting':
      return {
        labelKey: 'sse.connecting',
        dotClass: 'bg-sky-400 animate-pulse',
        pillClass: 'border-sky-500/30 bg-sky-500/10 text-sky-300',
      };
    case 'reconnecting':
      return {
        labelKey: 'sse.reconnecting',
        dotClass: 'bg-amber-400 animate-pulse',
        pillClass: 'border-amber-500/30 bg-amber-500/10 text-amber-300',
      };
    case 'error':
      return {
        labelKey: 'sse.error',
        dotClass: 'bg-rose-400',
        pillClass: 'border-rose-500/30 bg-rose-500/10 text-rose-300',
      };
    default:
      return {
        labelKey: 'sse.offline',
        dotClass: 'bg-slate-500',
        pillClass: 'border-slate-700 bg-slate-800/70 text-slate-400',
      };
  }
}

export function Layout({
  currentRoute,
  onNavigate,
  sseStatus,
  latestAlert,
  onDismissAlert,
  bridge,
  children,
}: LayoutProps) {
  const { user, onboarding, authConfig, logout } = useAuth();
  const { t } = useI18n();
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);

  const sseDisplay = getSseStatusDisplay(sseStatus);
  const sseLabel = t(sseDisplay.labelKey);
  const isOidc = authConfig?.authMode === AuthMode.OIDC;
  const userLabel = user?.displayName || user?.email || user?.oidcSub || t('auth.defaultUser');
  const needsOnboarding = Boolean(onboarding && (!onboarding.hasAiConfig || !onboarding.hasSources));
  const hasUrgentAlert = Boolean(latestAlert && latestAlert.items.length > 0);

  const handleOpenExternal = (url: string) => {
    if (!isAllowedExternalUrl(url)) {
      return;
    }
    void bridge.openExternalUrl(url).catch(() => {
      // Ignore blocked URL errors cleanly in UI click handler.
    });
  };

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100">
      <header className="sticky top-0 z-30 border-b border-slate-800/90 bg-slate-950/90 backdrop-blur">
        <div className="mx-auto flex max-w-7xl items-center justify-between gap-3 px-4 py-2.5 sm:px-6">
          <div className="flex min-w-0 shrink-0 items-center gap-2.5">
            <button
              type="button"
              onClick={() => onNavigate(APP_ROUTES.DAILY)}
              className="flex items-center gap-2.5 text-left focus:outline-none"
            >
              <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-gradient-to-br from-emerald-500/25 to-sky-500/20 text-emerald-300 ring-1 ring-emerald-500/30">
                <Sparkles className="h-4 w-4" />
              </span>
              <div>
                <div className="flex items-center gap-2">
                  <span className="whitespace-nowrap text-sm font-semibold tracking-tight text-white sm:text-base">
                    {t('common.appName')}
                  </span>
                  {bridge.runtime === PlatformRuntime.DESKTOP && (
                    <span className="inline-flex shrink-0 items-center gap-1 whitespace-nowrap rounded-md border border-sky-500/30 bg-sky-500/10 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-sky-300">
                      <Monitor className="h-3 w-3 shrink-0" />
                      {t('common.desktopBadge')}
                    </span>
                  )}
                </div>
              </div>
            </button>
          </div>

          <div className="flex shrink-0 items-center gap-2">
            <LanguageSwitcher />

            <span
              className={`inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full border px-2.5 py-1 text-xs font-medium ${sseDisplay.pillClass}`}
              title={t('sse.statusTooltip', { label: sseLabel })}
            >
              <span className={`h-2 w-2 shrink-0 rounded-full ${sseDisplay.dotClass}`} />
              <span className="hidden sm:inline">{sseLabel}</span>
            </span>

            {authConfig && (
              <span
                className={`hidden shrink-0 items-center gap-1 whitespace-nowrap rounded-full border px-2.5 py-1 text-xs font-medium lg:inline-flex ${
                  isOidc
                    ? 'border-sky-500/30 bg-sky-500/10 text-sky-300'
                    : 'border-amber-500/30 bg-amber-500/10 text-amber-300'
                }`}
              >
                <Shield className="h-3 w-3 shrink-0" />
                {isOidc ? t('auth.modeOidc') : t('auth.modeDevBypass')}
              </span>
            )}

            <span
              className="hidden max-w-[160px] truncate text-xs font-medium text-slate-300 md:inline"
              title={`${user?.email ?? userLabel}${authConfig ? ` (${isOidc ? t('auth.modeOidc') : t('auth.modeDevBypass')})` : ''}`}
            >
              {userLabel}
            </span>

            {isOidc && (
              <button
                type="button"
                onClick={logout}
                className="inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-lg border border-slate-700 bg-slate-900 px-2.5 py-1.5 text-xs font-medium text-slate-200 transition hover:border-slate-600 hover:bg-slate-800 hover:text-white"
                title={t('auth.signOutTitle')}
              >
                <LogOut className="h-3.5 w-3.5 shrink-0" />
                <span className="hidden sm:inline">{t('auth.logout')}</span>
              </button>
            )}

            <button
              type="button"
              onClick={() => setMobileMenuOpen((prev) => !prev)}
              className="inline-flex shrink-0 items-center justify-center rounded-lg border border-slate-800 bg-slate-900 p-2 text-slate-300 hover:bg-slate-800 hover:text-white md:hidden"
              aria-label={t('nav.toggleMenuAria')}
              aria-expanded={mobileMenuOpen}
            >
              {mobileMenuOpen ? <X className="h-4 w-4" /> : <Menu className="h-4 w-4" />}
            </button>
          </div>
        </div>

        <div className="hidden border-t border-slate-800/70 md:block">
          <nav
            className="mx-auto flex max-w-7xl items-center gap-1 overflow-x-auto px-4 py-1.5 sm:px-6"
            aria-label={t('nav.primaryAria')}
          >
            {NAV_ITEMS.map((item) => {
              const Icon = item.icon;
              const isActive = currentRoute === item.route;
              return (
                <button
                  key={item.route}
                  type="button"
                  onClick={() => onNavigate(item.route)}
                  aria-current={isActive ? 'page' : undefined}
                  className={`inline-flex shrink-0 items-center gap-2 whitespace-nowrap rounded-lg px-3 py-1.5 text-sm font-medium transition ${
                    isActive
                      ? 'bg-emerald-500/15 text-emerald-300 ring-1 ring-emerald-500/30'
                      : 'text-slate-300 hover:bg-slate-800/80 hover:text-white'
                  }`}
                >
                  <Icon className="h-4 w-4 shrink-0" />
                  {t(item.labelKey)}
                </button>
              );
            })}
          </nav>
        </div>

        {mobileMenuOpen && (
          <nav
            className="border-t border-slate-800 bg-slate-900/95 px-4 py-3 md:hidden"
            aria-label={t('nav.mobileAria')}
          >
            <div className="flex flex-col gap-1">
              {NAV_ITEMS.map((item) => {
                const Icon = item.icon;
                const isActive = currentRoute === item.route;
                return (
                  <button
                    key={item.route}
                    type="button"
                    onClick={() => {
                      onNavigate(item.route);
                      setMobileMenuOpen(false);
                    }}
                    aria-current={isActive ? 'page' : undefined}
                    className={`flex items-center gap-2.5 rounded-lg px-3 py-2 text-sm font-medium transition ${
                      isActive
                        ? 'bg-emerald-500/15 text-emerald-300 ring-1 ring-emerald-500/30'
                        : 'text-slate-300 hover:bg-slate-800 hover:text-white'
                    }`}
                  >
                    <Icon className="h-4 w-4" />
                    {t(item.labelKey)}
                  </button>
                );
              })}
              <div className="mt-2 flex items-center justify-between border-t border-slate-800 pt-2 text-xs text-slate-400">
                <span className="truncate">{userLabel}</span>
                {authConfig && <span>{isOidc ? t('auth.modeOidc') : t('auth.modeDevBypass')}</span>}
              </div>
            </div>
          </nav>
        )}
      </header>

      <div className="mx-auto max-w-7xl space-y-4 px-4 py-6 sm:px-6">
        {hasUrgentAlert && latestAlert && (
          <section
            role="status"
            aria-live="polite"
            className="rounded-xl border border-amber-500/40 bg-amber-500/10 p-4 text-amber-100 shadow-lg"
          >
            <div className="flex items-start justify-between gap-3">
              <div className="flex items-start gap-3">
                <AlertTriangle className="mt-0.5 h-5 w-5 shrink-0 text-amber-400" />
                <div className="space-y-2">
                  <div className="flex flex-wrap items-center gap-2">
                    <h2 className="text-sm font-semibold text-amber-200">
                      {t('layout.urgentAlertTitle', {
                        count: latestAlert.count,
                        unit:
                          latestAlert.count === 1
                            ? t('layout.alertUnitSingle')
                            : t('layout.alertUnitPlural'),
                      })}
                    </h2>
                  </div>
                  <ul className="space-y-1.5 text-xs text-amber-100/90">
                    {latestAlert.items.slice(0, 3).map((item) => {
                      const canOpen = isAllowedExternalUrl(item.canonicalUrl);
                      return (
                        <li key={item.itemId} className="flex flex-wrap items-center gap-2">
                          <span className="rounded bg-amber-500/20 px-1.5 py-0.5 font-medium text-amber-200">
                            {item.sourceName}
                          </span>
                          {canOpen ? (
                            <button
                              type="button"
                              onClick={() => handleOpenExternal(item.canonicalUrl)}
                              className="inline-flex items-center gap-1 text-left font-medium text-white underline decoration-amber-400/60 underline-offset-2 hover:text-amber-200"
                            >
                              <span>{item.title}</span>
                              <ExternalLink className="h-3 w-3 shrink-0" />
                            </button>
                          ) : (
                            <span className="font-medium text-white">{item.title}</span>
                          )}
                          {item.matchedKeywords.length > 0 && (
                            <span className="text-amber-300/90">
                              [{item.matchedKeywords.join(', ')}]
                            </span>
                          )}
                        </li>
                      );
                    })}
                  </ul>
                </div>
              </div>
              {onDismissAlert && (
                <button
                  type="button"
                  onClick={onDismissAlert}
                  className="rounded-lg p-1 text-amber-300 transition hover:bg-amber-500/20 hover:text-white"
                  aria-label={t('layout.dismissAlertAria')}
                >
                  <X className="h-4 w-4" />
                </button>
              )}
            </div>
          </section>
        )}

        {needsOnboarding && onboarding && (
          <section className="flex flex-col justify-between gap-4 rounded-xl border border-sky-500/30 bg-sky-500/10 p-4 sm:flex-row sm:items-center">
            <div className="space-y-1.5">
              <h2 className="text-sm font-semibold text-sky-200">{t('layout.completeSetup')}</h2>
              <div className="flex flex-wrap items-center gap-4 text-xs text-slate-300">
                <span className="inline-flex items-center gap-1.5">
                  <CheckCircle2
                    className={`h-4 w-4 ${onboarding.hasAiConfig ? 'text-emerald-400' : 'text-slate-500'}`}
                  />
                  {t('layout.aiProviderStatus', {
                    status: onboarding.hasAiConfig
                      ? t('layout.statusConfigured')
                      : t('layout.statusPending'),
                  })}
                </span>
                <span className="inline-flex items-center gap-1.5">
                  <CheckCircle2
                    className={`h-4 w-4 ${onboarding.hasSources ? 'text-emerald-400' : 'text-slate-500'}`}
                  />
                  {t('layout.sourcesStatus', {
                    status: onboarding.hasSources
                      ? t('layout.statusConfigured')
                      : t('layout.statusPending'),
                  })}
                </span>
              </div>
            </div>
            <button
              type="button"
              onClick={() => onNavigate(APP_ROUTES.SETTINGS)}
              className="inline-flex shrink-0 items-center gap-1.5 rounded-lg bg-sky-500 px-3.5 py-2 text-xs font-semibold text-slate-950 transition hover:bg-sky-400"
            >
              {t('layout.openSettings')}
              <ArrowRight className="h-3.5 w-3.5" />
            </button>
          </section>
        )}

        <main>{children}</main>
      </div>
    </div>
  );
}
