import { useEffect, useMemo, useState } from 'react';
import { Bell, CalendarClock, Cpu, Rss, ServerCog, Shield } from 'lucide-react';
import type { ApiClient } from '../api/client';
import { useOptionalAuth } from '../auth/AuthProvider';
import { AiConfigPanel } from '../components/settings/AiConfigPanel';
import { ChannelsPanel } from '../components/settings/ChannelsPanel';
import { DesktopMcpPanel } from '../components/settings/DesktopMcpPanel';
import { PlatformAdminPanel } from '../components/settings/PlatformAdminPanel';
import { ScheduleWatchlistPanel } from '../components/settings/ScheduleWatchlistPanel';
import { SourcesPanel } from '../components/settings/SourcesPanel';
import { type TranslationKey, useI18n } from '../i18n';
import type { PlatformBridge } from '../platform/bridge';

export const SETTINGS_TABS = {
  AI: 'ai',
  SOURCES: 'sources',
  CHANNELS: 'channels',
  SCHEDULE: 'schedule',
  MCP: 'mcp',
  PLATFORM: 'platform',
} as const;

export type SettingsTab = (typeof SETTINGS_TABS)[keyof typeof SETTINGS_TABS];

export interface SettingsPageProps {
  api: ApiClient;
  bridge: PlatformBridge;
  serverBaseUrl: string;
  onServerBaseUrlChange: (url: string) => void;
  initialTab?: SettingsTab;
  isAdmin?: boolean;
  platformXReady?: boolean;
}

const BASE_TAB_ITEMS: { id: SettingsTab; labelKey: TranslationKey; icon: typeof Cpu }[] = [
  { id: SETTINGS_TABS.AI, labelKey: 'settings.tabAi', icon: Cpu },
  { id: SETTINGS_TABS.SOURCES, labelKey: 'settings.tabSources', icon: Rss },
  { id: SETTINGS_TABS.CHANNELS, labelKey: 'settings.tabChannels', icon: Bell },
  { id: SETTINGS_TABS.SCHEDULE, labelKey: 'settings.tabSchedule', icon: CalendarClock },
  { id: SETTINGS_TABS.MCP, labelKey: 'settings.tabMcp', icon: ServerCog },
];

export function SettingsPage({
  api,
  bridge,
  serverBaseUrl,
  onServerBaseUrlChange,
  initialTab = SETTINGS_TABS.AI,
  isAdmin: isAdminOverride,
  platformXReady: platformXReadyOverride,
}: SettingsPageProps) {
  const { t } = useI18n();
  const auth = useOptionalAuth();
  const isAdmin =
    isAdminOverride ?? auth?.user?.role?.trim().toLowerCase() === 'admin';
  const platformXReady = platformXReadyOverride ?? auth?.platformXReady ?? false;

  const visibleTabs = useMemo(
    () =>
      isAdmin
        ? [
            ...BASE_TAB_ITEMS,
            { id: SETTINGS_TABS.PLATFORM, labelKey: 'settings.tabPlatform' as TranslationKey, icon: Shield },
          ]
        : BASE_TAB_ITEMS,
    [isAdmin],
  );

  const resolvedInitialTab =
    initialTab === SETTINGS_TABS.PLATFORM && !isAdmin ? SETTINGS_TABS.AI : initialTab;
  const [activeTab, setActiveTab] = useState<SettingsTab>(resolvedInitialTab);

  useEffect(() => {
    setActiveTab(initialTab === SETTINGS_TABS.PLATFORM && !isAdmin ? SETTINGS_TABS.AI : initialTab);
  }, [initialTab, isAdmin]);

  const activeTabItem = visibleTabs.find((tab) => tab.id === activeTab);

  return (
    <div className="space-y-6">
      <header>
        <h1 className="text-2xl font-bold tracking-tight text-white">{t('settings.title')}</h1>
        <p className="mt-1 text-sm text-slate-400">{t('settings.subtitle')}</p>
      </header>

      <nav
        role="tablist"
        aria-label={t('settings.tabsAria')}
        className="flex flex-wrap gap-2 border-b border-slate-800 pb-3"
      >
        {visibleTabs.map((tab) => {
          const Icon = tab.icon;
          const isActive = activeTab === tab.id;
          return (
            <button
              key={tab.id}
              type="button"
              role="tab"
              aria-selected={isActive}
              onClick={() => setActiveTab(tab.id)}
              className={`inline-flex items-center gap-2 rounded-lg px-3.5 py-2 text-sm font-medium transition ${
                isActive
                  ? 'bg-indigo-600 text-white shadow-sm'
                  : 'border border-slate-800 bg-slate-900/70 text-slate-300 hover:bg-slate-800 hover:text-white'
              }`}
            >
              <Icon className="h-4 w-4" />
              <span>{t(tab.labelKey)}</span>
            </button>
          );
        })}
      </nav>

      <div role="tabpanel" aria-label={activeTabItem ? t(activeTabItem.labelKey) : undefined}>
        {activeTab === SETTINGS_TABS.AI && <AiConfigPanel api={api} />}
        {activeTab === SETTINGS_TABS.SOURCES && (
          <SourcesPanel api={api} platformXReady={platformXReady} />
        )}
        {activeTab === SETTINGS_TABS.CHANNELS && <ChannelsPanel api={api} />}
        {activeTab === SETTINGS_TABS.SCHEDULE && <ScheduleWatchlistPanel api={api} />}
        {activeTab === SETTINGS_TABS.MCP && (
          <DesktopMcpPanel
            bridge={bridge}
            serverBaseUrl={serverBaseUrl}
            onServerBaseUrlChange={onServerBaseUrlChange}
          />
        )}
        {activeTab === SETTINGS_TABS.PLATFORM && isAdmin && <PlatformAdminPanel api={api} />}
      </div>
    </div>
  );
}
