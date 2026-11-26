import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  APP_ROUTES,
  DEFAULTS,
  STORAGE_KEYS,
  TAURI_COMMANDS,
  TAURI_EVENTS,
} from '../constants/routes';
import { AuthMode, McpAccessMode } from '../types/enums';

const { mockIsTauri, mockInvoke, mockListen } = vi.hoisted(() => ({
  mockIsTauri: vi.fn<() => boolean>(() => false),
  mockInvoke: vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>(),
  mockListen: vi.fn<
    (
      event: string,
      handler: (evt: { event: string; id: number; payload: unknown }) => void,
    ) => Promise<() => void>
  >(),
}));

vi.mock('@tauri-apps/api/core', () => ({
  isTauri: mockIsTauri,
  invoke: mockInvoke,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: mockListen,
}));

import {
  type McpBridgeStatus,
  PlatformRuntime,
  TauriPlatformBridge,
  WebPlatformBridge,
  createPlatformBridge,
} from '../platform/bridge';

describe('TauriPlatformBridge and runtime detection', () => {
  beforeEach(() => {
    window.localStorage.clear();
    window.sessionStorage.clear();
    mockIsTauri.mockReset().mockReturnValue(false);
    mockInvoke.mockReset();
    mockListen.mockReset();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('selects WebPlatformBridge when not in Tauri and TauriPlatformBridge when isTauri() is true', () => {
    mockIsTauri.mockReturnValue(false);
    const webBridge = createPlatformBridge();
    expect(webBridge).toBeInstanceOf(WebPlatformBridge);
    expect(webBridge.runtime).toBe(PlatformRuntime.WEB);

    mockIsTauri.mockReturnValue(true);
    const desktopBridge = createPlatformBridge();
    expect(desktopBridge).toBeInstanceOf(TauriPlatformBridge);
    expect(desktopBridge.runtime).toBe(PlatformRuntime.DESKTOP);
  });

  it('validates external URLs and invokes open_external_url and send_native_notification', async () => {
    mockInvoke.mockResolvedValue(undefined);
    const bridge = new TauriPlatformBridge();

    await expect(bridge.openExternalUrl('javascript:alert(1)')).rejects.toThrow(/Blocked unsafe/i);
    expect(mockInvoke).not.toHaveBeenCalled();

    await bridge.openExternalUrl('https://ai.google.dev/docs');
    expect(mockInvoke).toHaveBeenCalledWith(TAURI_COMMANDS.OPEN_EXTERNAL_URL, {
      url: 'https://ai.google.dev/docs',
    });

    await bridge.notify('Daily Digest Ready', '5 items');
    expect(mockInvoke).toHaveBeenCalledWith(TAURI_COMMANDS.SEND_NATIVE_NOTIFICATION, {
      title: 'Daily Digest Ready',
      body: '5 items',
    });

    // Notification IPC error is handled gracefully
    mockInvoke.mockRejectedValueOnce(new Error('Notification disabled'));
    await expect(bridge.notify('Alert', 'Body')).resolves.toBeUndefined();
  });

  it('syncs serverBaseUrl, mcpConfig, and runtime session via sync_desktop_runtime_config', async () => {
    const status: McpBridgeStatus = {
      running: true,
      endpointUrl: 'http://127.0.0.1:8765/mcp',
      bindHost: DEFAULTS.MCP_BIND_HOST,
      port: 8765,
      mode: McpAccessMode.READ_ONLY,
      tokenHint: '****ab12',
      lastError: null,
    };
    mockInvoke.mockResolvedValue(status);

    const bridge = new TauriPlatformBridge();
    await bridge.syncRuntimeSession({
      serverBaseUrl: 'https://digest.example.com/',
      accessToken: 'tok-desktop-1',
      authMode: AuthMode.OIDC,
    });

    expect(mockInvoke).toHaveBeenCalledWith(TAURI_COMMANDS.SYNC_DESKTOP_RUNTIME_CONFIG, {
      serverBaseUrl: 'https://digest.example.com',
      accessToken: 'tok-desktop-1',
      authMode: AuthMode.OIDC,
      mcpMode: McpAccessMode.READ_ONLY,
      mcpPort: DEFAULTS.DEFAULT_MCP_HTTP_PORT,
    });

    mockInvoke.mockClear();
    bridge.setServerBaseUrl('https://next.example.com///');
    expect(window.localStorage.getItem(STORAGE_KEYS.SERVER_BASE_URL)).toBe(
      'https://next.example.com',
    );
    expect(mockInvoke).toHaveBeenCalledWith(TAURI_COMMANDS.SYNC_DESKTOP_RUNTIME_CONFIG, {
      serverBaseUrl: 'https://next.example.com',
      accessToken: 'tok-desktop-1',
      authMode: AuthMode.OIDC,
      mcpMode: McpAccessMode.READ_ONLY,
      mcpPort: DEFAULTS.DEFAULT_MCP_HTTP_PORT,
    });

    mockInvoke.mockClear();
    bridge.setMcpConfig(McpAccessMode.READ_WRITE, 9123);
    expect(bridge.getMcpConfig()).toEqual({ mode: McpAccessMode.READ_WRITE, port: 9123 });
    expect(mockInvoke).toHaveBeenCalledWith(TAURI_COMMANDS.SYNC_DESKTOP_RUNTIME_CONFIG, {
      serverBaseUrl: 'https://next.example.com',
      accessToken: 'tok-desktop-1',
      authMode: AuthMode.OIDC,
      mcpMode: McpAccessMode.READ_WRITE,
      mcpPort: 9123,
    });
  });

  it('handles prepareOidcLoopback, openOidcSystemLogin, and oidcFetch via IPC', async () => {
    const bridge = new TauriPlatformBridge();

    mockInvoke.mockResolvedValueOnce('http://127.0.0.1:50123/callback');
    const redirectUri = await bridge.prepareOidcLoopback();
    expect(redirectUri).toBe('http://127.0.0.1:50123/callback');
    expect(mockInvoke).toHaveBeenCalledWith(TAURI_COMMANDS.PREPARE_OIDC_LOOPBACK);

    await expect(bridge.openOidcSystemLogin('javascript:alert(1)')).rejects.toThrow(
      /Blocked unsafe/i,
    );

    mockInvoke.mockResolvedValueOnce('http://127.0.0.1:50123/callback?code=c1&state=s1');
    const search = await bridge.openOidcSystemLogin(
      'https://auth.example.com/authorize?client_id=abc',
    );
    expect(search).toBe('?code=c1&state=s1');
    expect(mockInvoke).toHaveBeenCalledWith(TAURI_COMMANDS.OPEN_OIDC_SYSTEM_LOGIN, {
      authorizeUrl: 'https://auth.example.com/authorize?client_id=abc',
    });

    mockInvoke.mockResolvedValueOnce({
      status: 200,
      body: JSON.stringify({
        authorization_endpoint: 'https://auth.example.com/authorize',
        token_endpoint: 'https://auth.example.com/token',
      }),
    });
    const fetchFn = bridge.oidcFetch;
    const resp = await fetchFn('https://auth.example.com/.well-known/openid-configuration', {
      method: 'GET',
      headers: { accept: 'application/json' },
    });
    expect(resp.status).toBe(200);
    expect(await resp.json()).toEqual({
      authorization_endpoint: 'https://auth.example.com/authorize',
      token_endpoint: 'https://auth.example.com/token',
    });
    expect(mockInvoke).toHaveBeenCalledWith(TAURI_COMMANDS.OIDC_HTTP_REQUEST, {
      url: 'https://auth.example.com/.well-known/openid-configuration',
      method: 'GET',
      headers: { accept: 'application/json' },
    });
  });

  it('invokes MCP bridge status, local token, and token rotation commands', async () => {
    const bridge = new TauriPlatformBridge();
    const status: McpBridgeStatus = {
      running: true,
      endpointUrl: 'http://127.0.0.1:8765/mcp',
      bindHost: DEFAULTS.MCP_BIND_HOST,
      port: 8765,
      mode: McpAccessMode.READ_ONLY,
      tokenHint: '****c3d4',
      lastError: null,
    };

    mockInvoke
      .mockResolvedValueOnce(status)
      .mockResolvedValueOnce('mcp-secret-token-xyz')
      .mockResolvedValueOnce({ ...status, tokenHint: '****e5f6' });

    expect(await bridge.getMcpBridgeStatus()).toEqual(status);
    expect(await bridge.getMcpLocalToken()).toBe('mcp-secret-token-xyz');
    expect(await bridge.rotateMcpToken()).toEqual({ ...status, tokenHint: '****e5f6' });

    expect(mockInvoke).toHaveBeenNthCalledWith(1, TAURI_COMMANDS.GET_MCP_BRIDGE_STATUS);
    expect(mockInvoke).toHaveBeenNthCalledWith(2, TAURI_COMMANDS.GET_MCP_LOCAL_TOKEN);
    expect(mockInvoke).toHaveBeenNthCalledWith(3, TAURI_COMMANDS.ROTATE_MCP_TOKEN);
  });

  it('subscribes to desktop://navigate, filters invalid routes, and cleans up unlisten even if disposed early', async () => {
    const unlistenSpy = vi.fn();
    let capturedCallback:
      | ((evt: { event: string; id: number; payload: unknown }) => void)
      | undefined;

    mockListen.mockImplementation(async (eventName, cb) => {
      expect(eventName).toBe(TAURI_EVENTS.DESKTOP_NAVIGATE);
      capturedCallback = cb;
      return unlistenSpy;
    });

    const bridge = new TauriPlatformBridge();
    const handler = vi.fn();
    const unsubscribe = bridge.onDesktopNavigate(handler);

    await vi.waitFor(() => {
      expect(capturedCallback).toBeDefined();
    });

    // Invalid routes are ignored
    capturedCallback?.({
      event: TAURI_EVENTS.DESKTOP_NAVIGATE,
      id: 1,
      payload: { route: '/not-a-valid-route', params: { tab: 'mcp' } },
    });
    capturedCallback?.({
      event: TAURI_EVENTS.DESKTOP_NAVIGATE,
      id: 2,
      payload: { route: 'javascript:alert(1)' },
    });
    expect(handler).not.toHaveBeenCalled();

    // Valid APP_ROUTES value passes through with string-only params
    capturedCallback?.({
      event: TAURI_EVENTS.DESKTOP_NAVIGATE,
      id: 3,
      payload: { route: APP_ROUTES.SETTINGS, params: { tab: 'mcp', ignored: 123 } },
    });
    expect(handler).toHaveBeenCalledWith(APP_ROUTES.SETTINGS, { tab: 'mcp' });

    unsubscribe();
    expect(unlistenSpy).toHaveBeenCalledTimes(1);

    // Unsubscribing before listen() resolves should still call unlisten once resolved
    const lateUnlisten = vi.fn();
    let resolveListen: ((fn: () => void) => void) | undefined;
    mockListen.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveListen = resolve;
        }),
    );
    const earlyUnsub = bridge.onDesktopNavigate(vi.fn());
    earlyUnsub();
    resolveListen?.(lateUnlisten);
    await Promise.resolve();
    await Promise.resolve();
    expect(lateUnlisten).toHaveBeenCalledTimes(1);
  });
});
