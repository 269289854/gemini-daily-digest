import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from '../App';
import type { ApiClient } from '../api/client';
import { AiConfigPanel } from '../components/settings/AiConfigPanel';
import { ChannelsPanel } from '../components/settings/ChannelsPanel';
import { DesktopMcpPanel, buildMcpClientSnippet } from '../components/settings/DesktopMcpPanel';
import { ScheduleWatchlistPanel } from '../components/settings/ScheduleWatchlistPanel';
import { SourcesPanel } from '../components/settings/SourcesPanel';
import { APP_ROUTES, DEFAULTS, MCP_CLIENT_TARGETS, STORAGE_KEYS } from '../constants/routes';
import { SETTINGS_TABS, SettingsPage } from '../pages/SettingsPage';
import { type PlatformBridge, PlatformRuntime } from '../platform/bridge';
import type {
  NotificationChannelView,
  UserAiConfigView,
  UserScheduleRecord,
  UserSourceView,
  WatchlistRuleRecord,
} from '../types/api';
import {
  AlertPriority,
  AuthMode,
  DeliveryStatus,
  FetchOutcome,
  McpAccessMode,
  NotificationChannelType,
  SourceScope,
  SourceType,
} from '../types/enums';

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function createMockBridge(overrides: Partial<PlatformBridge> = {}): PlatformBridge {
  return {
    runtime: PlatformRuntime.WEB,
    openExternalUrl: vi.fn().mockResolvedValue(undefined),
    notify: vi.fn().mockResolvedValue(undefined),
    copyToClipboard: vi.fn().mockResolvedValue(undefined),
    getServerBaseUrl: vi.fn().mockReturnValue(''),
    setServerBaseUrl: vi.fn(),
    getMcpConfig: vi.fn().mockReturnValue({
      mode: McpAccessMode.READ_ONLY,
      port: DEFAULTS.DEFAULT_MCP_HTTP_PORT,
    }),
    setMcpConfig: vi.fn(),
    ...overrides,
  };
}

describe('SettingsPage, all 5 Settings Panels, and App Integration', () => {
  beforeEach(() => {
    window.localStorage.clear();
    window.sessionStorage.clear();
    window.history.replaceState(null, '', '/');
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('AiConfigPanel displays masked key hint, never prefills key, sends new key only when provided, and runs test', async () => {
    const view: UserAiConfigView = {
      providerLabel: 'DeepSeek',
      baseUrl: 'https://api.deepseek.com',
      apiKeyHint: '****1234',
      hasApiKey: true,
      model: 'deepseek-chat',
      systemPrompt: 'Summarize clearly.',
      temperature: 0.2,
      maxTokens: 4096,
      updatedAt: '2026-11-26T00:00:00Z',
    };

    const api = {
      getAiConfig: vi.fn().mockResolvedValue(view),
      updateAiConfig: vi.fn().mockResolvedValue(view),
      testAiConfig: vi.fn().mockResolvedValue({
        ok: true,
        providerLabel: 'DeepSeek',
        model: 'deepseek-chat',
        latencyMs: 145,
        message: 'Connection OK',
      }),
    } as unknown as ApiClient;

    render(<AiConfigPanel api={api} />);

    expect(await screen.findByText(/API Key Configured \(\*\*\*\*1234\)/)).toBeInTheDocument();
    const keyInput = screen.getByLabelText(/New API Key/i) as HTMLInputElement;
    expect(keyInput.value).toBe('');

    // Save without entering key -> apiKey omitted
    fireEvent.click(screen.getByRole('button', { name: /Save AI Config/i }));
    await waitFor(() => {
      expect(api.updateAiConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          providerLabel: 'DeepSeek',
          model: 'deepseek-chat',
          clearApiKey: false,
        }),
      );
    });
    expect(vi.mocked(api.updateAiConfig).mock.calls[0][0]).not.toHaveProperty('apiKey');

    // Enter optional new key and run connectivity test
    fireEvent.change(keyInput, { target: { value: 'test-tok-123' } });
    fireEvent.click(screen.getByRole('button', { name: /Test Connectivity/i }));
    await waitFor(() => {
      expect(api.testAiConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          apiKey: 'test-tok-123',
        }),
      );
    });
    expect(await screen.findByText(/Connectivity Passed/)).toBeInTheDocument();
    expect(screen.getByText('145 ms')).toBeInTheDocument();
  });

  it('AiConfigPanel auto-fetches model dropdown list, allows picking from dropdown, and supports manual custom input', async () => {
    const view: UserAiConfigView = {
      providerLabel: 'DeepSeek',
      baseUrl: 'https://api.deepseek.com',
      apiKeyHint: '****5678',
      hasApiKey: true,
      model: 'deepseek-chat',
      systemPrompt: 'Summarize clearly.',
      temperature: 0.2,
      maxTokens: 4096,
      updatedAt: '2026-11-26T00:00:00Z',
    };

    const api = {
      getAiConfig: vi.fn().mockResolvedValue(view),
      updateAiConfig: vi.fn().mockImplementation(async (cmd) => ({
        ...view,
        model: cmd.model,
      })),
      testAiConfig: vi.fn(),
      listAiModels: vi.fn().mockResolvedValue({
        models: [
          { id: 'deepseek-chat', ownedBy: 'deepseek' },
          { id: 'deepseek-reasoner', ownedBy: 'deepseek' },
        ],
        endpointUrl: 'https://api.deepseek.com/v1/models',
        latencyMs: 64,
      }),
    } as unknown as ApiClient;

    render(<AiConfigPanel api={api} />);

    // Auto-fetches available models on initial load when an API key is already stored.
    expect(
      await screen.findByText(/Loaded 2 models \(64 ms\)/i),
    ).toBeInTheDocument();
    expect(api.listAiModels).toHaveBeenCalledWith(
      expect.objectContaining({
        baseUrl: 'https://api.deepseek.com',
        clearApiKey: false,
      }),
    );

    // Clicking Refresh Models opens the dropdown listbox and allows selecting a fetched model.
    fireEvent.click(screen.getByRole('button', { name: /Refresh Models/i }));
    const reasonerOption = await screen.findByRole('option', { name: /deepseek-reasoner/i });
    fireEvent.click(reasonerOption);

    const modelInput = screen.getByRole('combobox') as HTMLInputElement;
    expect(modelInput.value).toBe('deepseek-reasoner');

    // Manual custom model typing still works and shows manual hint before saving.
    fireEvent.change(modelInput, { target: { value: 'custom-gemini-pro-preview' } });
    expect(
      screen.getByText(/Manual input: "custom-gemini-pro-preview"/i),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /Save AI Config/i }));
    await waitFor(() => {
      expect(api.updateAiConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          model: 'custom-gemini-pro-preview',
        }),
      );
    });
  });

  it('SourcesPanel handles preset import, locks X sources to user_private, and supports export/import/test', async () => {
    const publicSrc: UserSourceView = {
      id: 'src-1',
      subscriptionId: 'sub-1',
      sourceType: SourceType.RSS,
      scope: SourceScope.SHARED_PUBLIC,
      canonicalKey: 'google-ai-blog',
      customName: 'Google AI Blog',
      defaultName: 'Google AI Blog',
      category: 'Official',
      handleOrUrl: 'https://blog.google/technology/ai/rss/',
      enabled: true,
      hasCredentials: false,
      configHint: null,
      lastFetchedAt: null,
      nextPollAt: null,
      lastStatus: FetchOutcome.UPDATED,
      lastError: null,
      consecutiveFailures: 0,
      createdAt: '2026-11-26T00:00:00Z',
    };

    const api = {
      listSources: vi.fn().mockResolvedValue([publicSrc]),
      importPresetSources: vi.fn().mockResolvedValue({ importedCount: 5, sources: [publicSrc] }),
      createSource: vi.fn().mockResolvedValue({
        ...publicSrc,
        id: 'src-x',
        sourceType: SourceType.X_ACCOUNT,
        scope: SourceScope.USER_PRIVATE,
        customName: 'Gemini X Feed',
        handleOrUrl: '@example_handle',
      }),
      updateSource: vi.fn().mockResolvedValue(publicSrc),
      deleteSource: vi.fn().mockResolvedValue(undefined),
      testSource: vi.fn().mockResolvedValue({
        sourceId: 'src-1',
        outcome: FetchOutcome.UPDATED,
        itemsFetched: 3,
        sampleTitles: ['Sample Post 1'],
        errorMessage: null,
      }),
      exportSources: vi.fn().mockResolvedValue({
        version: 1,
        exportedAt: '2026-11-26T08:00:00Z',
        sources: [
          {
            sourceType: SourceType.RSS,
            scope: SourceScope.SHARED_PUBLIC,
            canonicalKey: 'google-ai-blog',
            customName: 'Google AI Blog',
            category: 'Official',
            handleOrUrl: 'https://blog.google/technology/ai/rss/',
            enabled: true,
          },
        ],
      }),
      importSources: vi.fn().mockResolvedValue({ importedCount: 1, sources: [publicSrc] }),
    } as unknown as ApiClient;

    render(<SourcesPanel api={api} />);
    expect(await screen.findByText('Google AI Blog')).toBeInTheDocument();

    // Preset import
    fireEvent.click(screen.getByRole('button', { name: /Import Preset Pack/i }));
    await waitFor(() => {
      expect(api.importPresetSources).toHaveBeenCalledWith({ includeXTemplates: false });
    });

    // Selecting X_ACCOUNT locks scope to user_private
    const typeSelect = screen.getByLabelText(/Source Type/i) as HTMLSelectElement;
    const scopeSelect = screen.getByLabelText(/^Scope$/i) as HTMLSelectElement;
    expect(scopeSelect.disabled).toBe(false);
    fireEvent.change(typeSelect, { target: { value: SourceType.X_ACCOUNT } });
    expect(scopeSelect.disabled).toBe(true);
    expect(scopeSelect.value).toBe(SourceScope.USER_PRIVATE);

    fireEvent.change(screen.getByLabelText(/Handle or URL/i), {
      target: { value: '@example_handle' },
    });
    fireEvent.click(screen.getByRole('button', { name: /Add Source/i }));
    await waitFor(() => {
      expect(api.createSource).toHaveBeenCalledWith(
        expect.objectContaining({
          sourceType: SourceType.X_ACCOUNT,
          scope: SourceScope.USER_PRIVATE,
          handleOrUrl: '@example_handle',
        }),
      );
    });

    // Live source test
    const testButtons = screen.getAllByRole('button', { name: /^Test$/i });
    fireEvent.click(testButtons[testButtons.length - 1]);
    expect(await screen.findByText(/Live Test: updated • 3 items fetched/)).toBeInTheDocument();

    // Export & Import JSON
    fireEvent.click(screen.getByRole('button', { name: /Export Sources JSON/i }));
    await waitFor(() => {
      expect((screen.getByLabelText('Exported sources JSON') as HTMLTextAreaElement).value).toContain(
        'google-ai-blog',
      );
    });

    const importTextarea = screen.getByLabelText('Import sources JSON');
    fireEvent.change(importTextarea, {
      target: {
        value: JSON.stringify({
          version: 1,
          exportedAt: '2026-11-26T00:00:00Z',
          sources: [],
        }),
      },
    });
    fireEvent.click(screen.getByRole('button', { name: /Import Sources JSON/i }));
    await waitFor(() => {
      expect(api.importSources).toHaveBeenCalledWith(
        expect.objectContaining({ version: 1, sources: [] }),
      );
    });
  });

  it('ChannelsPanel renders channel types, masked hint on edit, create, and test', async () => {
    const ch1: NotificationChannelView = {
      id: 'ch-1',
      channelType: NotificationChannelType.TELEGRAM,
      name: 'Ops Telegram',
      enabled: true,
      configHint: 'chat=-100***7890',
      triggerOnDaily: true,
      triggerOnAlert: true,
      createdAt: '2026-11-26T00:00:00Z',
    };

    const api = {
      listChannels: vi.fn().mockResolvedValue([ch1]),
      createChannel: vi.fn().mockResolvedValue({
        id: 'ch-2',
        channelType: NotificationChannelType.WEBHOOK,
        name: 'Custom Hook',
        enabled: true,
        configHint: 'https://hooks.example.com/***',
        triggerOnDaily: true,
        triggerOnAlert: true,
        createdAt: '2026-11-26T01:00:00Z',
      }),
      updateChannel: vi.fn().mockResolvedValue(ch1),
      deleteChannel: vi.fn().mockResolvedValue(undefined),
      testChannel: vi.fn().mockResolvedValue({
        channelId: 'ch-1',
        channelType: NotificationChannelType.TELEGRAM,
        status: DeliveryStatus.SENT,
        errorMessage: null,
        sentAt: '2026-11-26T08:00:00Z',
      }),
    } as unknown as ApiClient;

    render(<ChannelsPanel api={api} />);
    expect(await screen.findByText('Ops Telegram')).toBeInTheDocument();
    expect(screen.getByText(/Config hint: chat=-100\*\*\*7890/)).toBeInTheDocument();

    // Edit shows masked hint without prefilling secret
    fireEvent.click(screen.getByRole('button', { name: /^Edit$/i }));
    expect(screen.getByText(/Stored configuration hint:/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /^Cancel$/i }));

    // Test Send
    fireEvent.click(screen.getByRole('button', { name: /Test Send/i }));
    expect(await screen.findByText(/Test Send: sent \(telegram\)/)).toBeInTheDocument();

    // Switch channel type to Webhook and create
    fireEvent.change(screen.getByLabelText(/Channel Type/i), {
      target: { value: NotificationChannelType.WEBHOOK },
    });
    fireEvent.change(screen.getByLabelText(/Channel Name/i), {
      target: { value: 'Custom Hook' },
    });
    fireEvent.change(screen.getByLabelText(/Webhook URL/i), {
      target: { value: 'https://hooks.example.com/digest' },
    });
    fireEvent.click(screen.getByRole('button', { name: /Add Channel/i }));
    await waitFor(() => {
      expect(api.createChannel).toHaveBeenCalledWith(
        expect.objectContaining({
          channelType: NotificationChannelType.WEBHOOK,
          name: 'Custom Hook',
          config: { url: 'https://hooks.example.com/digest', secret: undefined },
        }),
      );
    });
  });

  it('ScheduleWatchlistPanel submits all 6 schedule fields and always sends excludeKeywords array', async () => {
    const schedule: UserScheduleRecord = {
      userId: 'u1',
      timezone: 'Asia/Shanghai',
      dailyTime: '08:30',
      emailEnabled: true,
      sendWhenEmpty: false,
      alertScanEnabled: true,
      alertIntervalMinutes: 30,
      lastDailyRunDate: null,
      lastAlertScanAt: null,
    };

    const existingRule: WatchlistRuleRecord = {
      id: 'wl-1',
      userId: 'u1',
      name: 'Gemini Releases',
      keywords: ['gemini', 'sdk'],
      excludeKeywords: ['rumor'],
      priority: AlertPriority.HIGH,
      notifyImmediately: true,
      enabled: true,
      createdAt: '2026-11-26T00:00:00Z',
    };

    const api = {
      getSchedule: vi.fn().mockResolvedValue(schedule),
      updateSchedule: vi.fn().mockResolvedValue({ ...schedule, dailyTime: '09:00' }),
      listWatchlist: vi.fn().mockResolvedValue([existingRule]),
      createWatchlistRule: vi.fn().mockResolvedValue({
        ...existingRule,
        id: 'wl-2',
        name: 'Vertex Updates',
        keywords: ['vertex'],
        excludeKeywords: [],
      }),
      updateWatchlistRule: vi.fn().mockResolvedValue({ ...existingRule, enabled: false }),
      deleteWatchlistRule: vi.fn().mockResolvedValue(undefined),
    } as unknown as ApiClient;

    render(<ScheduleWatchlistPanel api={api} />);
    expect(await screen.findByDisplayValue('08:30')).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText(/Daily Time/i), { target: { value: '09:00' } });
    fireEvent.click(screen.getByRole('button', { name: /Save Schedule/i }));
    await waitFor(() => {
      expect(api.updateSchedule).toHaveBeenCalledWith({
        timezone: 'Asia/Shanghai',
        dailyTime: '09:00',
        emailEnabled: true,
        sendWhenEmpty: false,
        alertScanEnabled: true,
        alertIntervalMinutes: 30,
      });
    });

    // Create rule with empty excludeKeywords -> must send excludeKeywords: []
    fireEvent.change(screen.getByLabelText(/Rule Name/i), { target: { value: 'Vertex Updates' } });
    fireEvent.change(screen.getByLabelText(/^Keywords/i), { target: { value: 'vertex' } });
    fireEvent.click(screen.getByRole('button', { name: /Add Watchlist Rule/i }));
    await waitFor(() => {
      expect(api.createWatchlistRule).toHaveBeenCalledWith({
        name: 'Vertex Updates',
        keywords: ['vertex'],
        excludeKeywords: [],
        priority: AlertPriority.HIGH,
        notifyImmediately: true,
        enabled: true,
      });
    });

    // Toggle rule -> must also preserve/send excludeKeywords array
    fireEvent.click(screen.getAllByRole('button', { name: /^Enabled$/i })[0]);
    await waitFor(() => {
      expect(api.updateWatchlistRule).toHaveBeenCalledWith(
        'wl-2',
        expect.objectContaining({
          excludeKeywords: [],
          enabled: false,
        }),
      );
    });
  });

  it('DesktopMcpPanel generates valid secret-free JSON snippets for all 5 MCP targets and persists config', async () => {
    const bridge = createMockBridge();
    const onServerBaseUrlChange = vi.fn();

    for (const target of Object.values(MCP_CLIENT_TARGETS)) {
      const snippet = buildMcpClientSnippet(target, {
        mcpPort: 8765,
        serverBaseUrl: 'https://digest.example.com',
      });
      const parsed = JSON.parse(snippet) as Record<string, unknown>;
      expect(parsed).toBeTruthy();
      expect(snippet).toContain('YOUR_MCP_TOKEN');
      expect(snippet).not.toContain('sk-');
    }

    render(
      <DesktopMcpPanel
        bridge={bridge}
        serverBaseUrl=""
        onServerBaseUrlChange={onServerBaseUrlChange}
      />,
    );

    fireEvent.change(screen.getByLabelText(/Backend Server URL/i), {
      target: { value: 'https://digest.example.com/' },
    });
    fireEvent.click(screen.getByRole('button', { name: /Save Server URL/i }));
    expect(bridge.setServerBaseUrl).toHaveBeenCalledWith('https://digest.example.com');
    expect(onServerBaseUrlChange).toHaveBeenCalledWith('https://digest.example.com');

    fireEvent.change(screen.getByLabelText(/MCP Access Mode/i), {
      target: { value: McpAccessMode.READ_WRITE },
    });
    fireEvent.change(screen.getByLabelText(/Local HTTP Port/i), { target: { value: '9000' } });
    fireEvent.click(screen.getByRole('button', { name: /Save MCP Settings/i }));
    expect(bridge.setMcpConfig).toHaveBeenCalledWith(McpAccessMode.READ_WRITE, 9000);

    const copyButtons = screen.getAllByRole('button', { name: /Copy JSON/i });
    expect(copyButtons).toHaveLength(5);
    fireEvent.click(copyButtons[0]);
    await waitFor(() => {
      expect(bridge.copyToClipboard).toHaveBeenCalledTimes(1);
    });
  });

  it('SettingsPage switches across all 5 accessible tabs', async () => {
    const bridge = createMockBridge();
    const api = {
      getAiConfig: vi.fn().mockResolvedValue({
        providerLabel: 'DeepSeek',
        baseUrl: 'https://api.deepseek.com',
        apiKeyHint: null,
        hasApiKey: false,
        model: 'deepseek-chat',
        systemPrompt: 'Prompt',
        temperature: 0.2,
        maxTokens: 2048,
        updatedAt: null,
      }),
      listSources: vi.fn().mockResolvedValue([]),
      listChannels: vi.fn().mockResolvedValue([]),
      getSchedule: vi.fn().mockResolvedValue({
        userId: 'u1',
        timezone: 'UTC',
        dailyTime: '08:00',
        emailEnabled: true,
        sendWhenEmpty: false,
        alertScanEnabled: false,
        alertIntervalMinutes: 60,
        lastDailyRunDate: null,
        lastAlertScanAt: null,
      }),
      listWatchlist: vi.fn().mockResolvedValue([]),
    } as unknown as ApiClient;

    render(
      <SettingsPage
        api={api}
        bridge={bridge}
        serverBaseUrl=""
        onServerBaseUrlChange={vi.fn()}
        initialTab={SETTINGS_TABS.AI}
      />,
    );

    expect(await screen.findByText('AI Provider Configuration')).toBeInTheDocument();
    expect(screen.queryByRole('tab', { name: /Platform \(Admin\)/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: /Desktop & MCP/i }));
    expect(screen.getByText('MCP Client Config Snippets')).toBeInTheDocument();

    cleanup();

    render(
      <SettingsPage
        api={api}
        bridge={bridge}
        serverBaseUrl=""
        onServerBaseUrlChange={vi.fn()}
        initialTab={SETTINGS_TABS.AI}
        isAdmin
      />,
    );
    expect(await screen.findByRole('tab', { name: /Platform \(Admin\)/i })).toBeInTheDocument();
  });

  it('App renders Sign-In screen with Server Base URL override in OIDC mode and supports SPA + Desktop navigation in DEV_BYPASS mode', async () => {
    // Part 1: Unauthenticated OIDC Sign-In card + Server Base URL override
    const oidcFetch = vi.fn<typeof fetch>().mockImplementation(async () =>
      jsonResponse({
        authMode: AuthMode.OIDC,
        issuerUrl: 'https://auth.example.com/o/digest',
        clientId: 'client-1',
        scopes: ['openid'],
      }),
    );

    const desktopBridge = createMockBridge({ runtime: PlatformRuntime.DESKTOP });
    render(<App bridge={desktopBridge} fetchImpl={oidcFetch} />);

    expect(await screen.findByRole('button', { name: /Sign in with OIDC/i })).toBeInTheDocument();
    const serverInput = screen.getByLabelText('Server Base URL');
    fireEvent.change(serverInput, { target: { value: 'https://digest.example.com/' } });
    fireEvent.click(screen.getByRole('button', { name: /Apply URL/i }));
    expect(desktopBridge.setServerBaseUrl).toHaveBeenCalledWith('https://digest.example.com');
    expect(await screen.findByText(/Server URL set to https:\/\/digest\.example\.com/)).toBeInTheDocument();

    cleanup();

    // Part 2: Authenticated DEV_BYPASS SPA navigation + onDesktopNavigate
    let desktopNavHandler: ((route: string, params?: Record<string, string>) => void) | undefined;
    const devBridge = createMockBridge({
      runtime: PlatformRuntime.DESKTOP,
      onDesktopNavigate: (handler) => {
        desktopNavHandler = handler;
        return () => {
          desktopNavHandler = undefined;
        };
      },
    });

    const sseBody = new ReadableStream<Uint8Array>({
      start() {
        // Keep stream open silently during smoke test
      },
    });

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
            id: 'u-dev',
            oidcSub: 'dev-bypass',
            email: 'dev@example.com',
            displayName: 'Dev User',
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
      if (url.endsWith('/api/v1/events/stream')) {
        return new Response(sseBody, {
          status: 200,
          headers: { 'content-type': 'text/event-stream' },
        });
      }
      if (url.includes('/api/v1/digests?')) {
        return jsonResponse({ items: [], page: 1, pageSize: 20, total: 0, totalPages: 0 });
      }
      if (url.includes('/api/v1/digests/')) {
        return jsonResponse(
          { code: 'NOT_FOUND', message: 'Not found', requestId: 'req-1' },
          404,
        );
      }
      if (url.endsWith('/api/v1/settings/sources')) {
        return jsonResponse([]);
      }
      if (url.includes('/api/v1/items')) {
        return jsonResponse({ items: [], page: 1, pageSize: 20, total: 0, totalPages: 0 });
      }
      if (url.endsWith('/api/v1/runs/stats')) {
        return jsonResponse({
          totalRuns: 0,
          succeededRuns: 0,
          failedRuns: 0,
          totalPromptTokens: 0,
          totalCompletionTokens: 0,
          deliveriesSent: 0,
          deliveriesFailed: 0,
          sourceHealth: [],
        });
      }
      if (url.includes('/api/v1/runs')) {
        return jsonResponse({ items: [], page: 1, pageSize: 20, total: 0, totalPages: 0 });
      }
      if (url.endsWith('/api/v1/settings/ai')) {
        return jsonResponse({
          providerLabel: 'DeepSeek',
          baseUrl: 'https://api.deepseek.com',
          apiKeyHint: null,
          hasApiKey: false,
          model: 'deepseek-chat',
          systemPrompt: 'Prompt',
          temperature: 0.2,
          maxTokens: 4096,
          updatedAt: null,
        });
      }
      return jsonResponse({});
    });

    window.localStorage.removeItem(STORAGE_KEYS.SERVER_BASE_URL);
    render(<App bridge={devBridge} fetchImpl={devFetch} />);

    expect(await screen.findByRole('heading', { name: 'Daily Digest' })).toBeInTheDocument();
    expect(window.location.pathname).toBe('/');

    fireEvent.click(screen.getByRole('button', { name: /^Sources$/i }));
    expect(await screen.findByRole('heading', { name: 'Sources Explorer' })).toBeInTheDocument();
    expect(window.location.pathname).toBe(APP_ROUTES.SOURCES);

    fireEvent.click(screen.getByRole('button', { name: /^History$/i }));
    expect(await screen.findByRole('heading', { name: /History & Bookmarks/i })).toBeInTheDocument();
    expect(window.location.pathname).toBe(APP_ROUTES.HISTORY);

    fireEvent.click(screen.getByRole('button', { name: /Runs & Stats/i }));
    expect(await screen.findByRole('heading', { name: /Runs & Statistics/i })).toBeInTheDocument();
    expect(window.location.pathname).toBe(APP_ROUTES.RUNS);

    fireEvent.click(screen.getByRole('button', { name: /^Settings$/i }));
    expect(await screen.findByRole('heading', { name: /^Settings$/i })).toBeInTheDocument();
    expect(window.location.pathname).toBe(APP_ROUTES.SETTINGS);

    // Desktop MCP navigation hook
    expect(desktopNavHandler).toBeDefined();
    desktopNavHandler?.(APP_ROUTES.SETTINGS, { tab: SETTINGS_TABS.MCP });
    expect(await screen.findByText('MCP Client Config Snippets')).toBeInTheDocument();
    expect(window.location.search).toContain('tab=mcp');
  });

  it('DesktopMcpPanel displays desktop bridge status, copies local token without rendering it, and rotates token', async () => {
    const desktopBridge = createMockBridge({
      runtime: PlatformRuntime.DESKTOP,
      getMcpBridgeStatus: vi.fn().mockResolvedValue({
        running: true,
        endpointUrl: 'http://127.0.0.1:8765/mcp',
        bindHost: '127.0.0.1',
        port: 8765,
        mode: McpAccessMode.READ_ONLY,
        tokenHint: '****9f2a',
        lastError: null,
      }),
      getMcpLocalToken: vi.fn().mockResolvedValue('local-mcp-secret-token-value'),
      rotateMcpToken: vi.fn().mockResolvedValue({
        running: true,
        endpointUrl: 'http://127.0.0.1:8765/mcp',
        bindHost: '127.0.0.1',
        port: 8765,
        mode: McpAccessMode.READ_ONLY,
        tokenHint: '****b81c',
        lastError: null,
      }),
    });

    const { container } = render(
      <DesktopMcpPanel
        bridge={desktopBridge}
        serverBaseUrl="https://digest.example.com"
        onServerBaseUrlChange={vi.fn()}
      />,
    );

    expect(await screen.findByText('Desktop Bridge Status')).toBeInTheDocument();
    expect(screen.getByText('Running')).toBeInTheDocument();
    expect(screen.getByText('http://127.0.0.1:8765/mcp')).toBeInTheDocument();
    expect(screen.getByText('****9f2a')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /Copy Local Token/i }));
    await waitFor(() => {
      expect(desktopBridge.getMcpLocalToken).toHaveBeenCalledTimes(1);
      expect(desktopBridge.copyToClipboard).toHaveBeenCalledWith('local-mcp-secret-token-value');
    });
    expect(await screen.findByText(/Copied local MCP Bearer token/i)).toBeInTheDocument();
    expect(container.textContent).not.toContain('local-mcp-secret-token-value');

    fireEvent.click(screen.getByRole('button', { name: /Rotate Token/i }));
    await waitFor(() => {
      expect(desktopBridge.rotateMcpToken).toHaveBeenCalledTimes(1);
    });
    expect(await screen.findByText('****b81c')).toBeInTheDocument();
  });

  it('App triggers native desktop notification on digest_ready SSE event in DESKTOP runtime', async () => {
    const encoder = new TextEncoder();
    const sseBody = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(
          encoder.encode(
            'event: digest_ready\ndata: {"digestId":"d-1","digestDate":"2026-11-26","title":"Gemini Daily Briefing","itemCount":7,"highlightedCount":2,"degradedFallback":false}\n\n',
          ),
        );
      },
    });

    const desktopBridge = createMockBridge({
      runtime: PlatformRuntime.DESKTOP,
    });

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
            id: 'u-dev',
            oidcSub: 'dev-bypass',
            email: 'dev@example.com',
            displayName: 'Dev User',
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
      if (url.endsWith('/api/v1/events/stream')) {
        return new Response(sseBody, {
          status: 200,
          headers: { 'content-type': 'text/event-stream' },
        });
      }
      if (url.includes('/api/v1/digests?')) {
        return jsonResponse({ items: [], page: 1, pageSize: 20, total: 0, totalPages: 0 });
      }
      if (url.includes('/api/v1/digests/')) {
        return jsonResponse(
          { code: 'NOT_FOUND', message: 'Not found', requestId: 'req-1' },
          404,
        );
      }
      return jsonResponse({});
    });

    render(<App bridge={desktopBridge} fetchImpl={devFetch} />);

    await waitFor(() => {
      expect(desktopBridge.notify).toHaveBeenCalledWith(
        'Daily Digest Ready',
        'Gemini Daily Briefing (7 items)',
      );
    });
  });
});
