/**
 * Adapter pattern isolating Web Browser vs Tauri 2 Desktop runtime differences.
 * UI pages never inspect `window.__TAURI__` directly.
 */

import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { normalizeBaseUrl } from '../api/client';
import {
  APP_ROUTES,
  DEFAULTS,
  HTTP_HEADERS,
  STORAGE_KEYS,
  TAURI_COMMANDS,
  TAURI_EVENTS,
} from '../constants/routes';
import { type AuthMode, McpAccessMode } from '../types/enums';

export const PlatformRuntime = {
  WEB: 'web',
  DESKTOP: 'desktop',
} as const;
export type PlatformRuntime = (typeof PlatformRuntime)[keyof typeof PlatformRuntime];

export interface McpConfig {
  mode: McpAccessMode;
  port: number;
}

export interface McpBridgeStatus {
  running: boolean;
  endpointUrl: string;
  bindHost: string;
  port: number;
  mode: McpAccessMode;
  tokenHint: string;
  lastError: string | null;
}

export interface RuntimeSessionSync {
  serverBaseUrl: string;
  accessToken: string | null;
  authMode: AuthMode | null;
}

export interface PlatformBridge {
  readonly runtime: PlatformRuntime;
  openExternalUrl(url: string): Promise<void>;
  notify(title: string, body: string): Promise<void>;
  copyToClipboard(text: string): Promise<void>;
  getServerBaseUrl(): string;
  setServerBaseUrl(url: string): void;
  getMcpConfig(): McpConfig;
  setMcpConfig(mode: McpAccessMode, port: number): void;
  prepareOidcLoopback?(): Promise<string>;
  openOidcSystemLogin?(authorizeUrl: string): Promise<string>;
  oidcFetch?: typeof fetch;
  syncRuntimeSession?(session: RuntimeSessionSync): Promise<void>;
  getMcpBridgeStatus?(): Promise<McpBridgeStatus>;
  getMcpLocalToken?(): Promise<string>;
  rotateMcpToken?(): Promise<McpBridgeStatus>;
  onDesktopNavigate?(handler: (route: string, params?: Record<string, string>) => void): () => void;
}

const VALID_MCP_MODES: ReadonlySet<string> = new Set(Object.values(McpAccessMode));
const VALID_APP_ROUTES: ReadonlySet<string> = new Set(Object.values(APP_ROUTES));

/**
 * Validates that an external URL uses ONLY `http:` or `https:` with a valid host.
 * Rejects `javascript:`, `data:`, `vbscript:`, `file:`, relative paths, and malformed URLs.
 */
export function isAllowedExternalUrl(url: string): boolean {
  if (typeof url !== 'string') {
    return false;
  }
  const trimmed = url.trim();
  if (!trimmed) {
    return false;
  }
  try {
    const parsed = new URL(trimmed);
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      return false;
    }
    return parsed.hostname.length > 0;
  } catch {
    return false;
  }
}

function readLocalStorage(key: string): string | null {
  if (typeof window === 'undefined') {
    return null;
  }
  try {
    return window.localStorage?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function writeLocalStorage(key: string, value: string | null): void {
  if (typeof window === 'undefined') {
    return;
  }
  try {
    if (value === null) {
      window.localStorage?.removeItem(key);
    } else {
      window.localStorage?.setItem(key, value);
    }
  } catch {
    // Ignore storage quota or private-browsing errors.
  }
}

function normalizeMcpMode(raw: string | null): McpAccessMode {
  if (raw && VALID_MCP_MODES.has(raw)) {
    return raw as McpAccessMode;
  }
  return McpAccessMode.READ_ONLY;
}

function normalizeMcpPort(raw: string | number | null): number {
  const num = typeof raw === 'number' ? raw : raw !== null ? Number.parseInt(raw, 10) : Number.NaN;
  if (Number.isInteger(num) && num >= 1 && num <= 65535) {
    return num;
  }
  return DEFAULTS.DEFAULT_MCP_HTTP_PORT;
}

export function extractOidcCallbackSearch(rawResult: string): string {
  if (typeof rawResult !== 'string') {
    return '';
  }
  const trimmed = rawResult.trim();
  if (!trimmed) {
    return '';
  }
  if (trimmed.startsWith('?')) {
    return trimmed;
  }
  try {
    const parsed = new URL(trimmed);
    return parsed.search;
  } catch {
    return trimmed.includes('code=') || trimmed.includes('error=') ? `?${trimmed}` : '';
  }
}

async function copyTextToClipboard(text: string): Promise<void> {
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // Fall through to execCommand fallback in restricted/non-HTTPS contexts.
    }
  }

  if (typeof document === 'undefined' || !document.body) {
    return;
  }

  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.setAttribute('readonly', '');
  textarea.style.position = 'fixed';
  textarea.style.opacity = '0';
  textarea.style.pointerEvents = 'none';
  document.body.appendChild(textarea);
  try {
    textarea.select();
    if (typeof document.execCommand === 'function') {
      document.execCommand('copy');
    }
  } catch {
    // Ignore clipboard fallback failures in headless environments.
  } finally {
    document.body.removeChild(textarea);
  }
}

function readStoredServerBaseUrl(): string {
  const raw = readLocalStorage(STORAGE_KEYS.SERVER_BASE_URL);
  if (!raw) {
    return '';
  }
  try {
    return normalizeBaseUrl(raw);
  } catch {
    return '';
  }
}

function writeStoredServerBaseUrl(url: string): string {
  const normalized = normalizeBaseUrl(url);
  writeLocalStorage(STORAGE_KEYS.SERVER_BASE_URL, normalized ? normalized : null);
  return normalized;
}

function readStoredMcpConfig(): McpConfig {
  const mode = normalizeMcpMode(readLocalStorage(STORAGE_KEYS.MCP_ACCESS_MODE));
  const port = normalizeMcpPort(readLocalStorage(STORAGE_KEYS.MCP_PORT));
  return { mode, port };
}

function writeStoredMcpConfig(mode: McpAccessMode, port: number): McpConfig {
  const safeMode = VALID_MCP_MODES.has(mode) ? mode : McpAccessMode.READ_ONLY;
  const safePort = normalizeMcpPort(port);
  writeLocalStorage(STORAGE_KEYS.MCP_ACCESS_MODE, safeMode);
  writeLocalStorage(STORAGE_KEYS.MCP_PORT, String(safePort));
  return { mode: safeMode, port: safePort };
}

function sanitizeNavigateParams(raw: unknown): Record<string, string> | undefined {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    return undefined;
  }
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof value === 'string') {
      result[key] = value;
    }
  }
  return Object.keys(result).length > 0 ? result : undefined;
}

function isTauriRuntime(): boolean {
  if (typeof window === 'undefined') {
    return false;
  }
  try {
    return isTauri();
  } catch {
    return false;
  }
}

export class WebPlatformBridge implements PlatformBridge {
  public readonly runtime = PlatformRuntime.WEB;

  public async openExternalUrl(url: string): Promise<void> {
    if (!isAllowedExternalUrl(url)) {
      throw new Error('Blocked unsafe external URL: only http and https protocols are allowed');
    }
    if (typeof window !== 'undefined' && typeof window.open === 'function') {
      window.open(url.trim(), '_blank', 'noopener,noreferrer');
    }
  }

  public async notify(title: string, body: string): Promise<void> {
    try {
      if (typeof Notification !== 'undefined' && Notification.permission === 'granted') {
        new Notification(title, { body });
      }
    } catch {
      // Ignore notification failures in headless or restricted contexts.
    }
  }

  public async copyToClipboard(text: string): Promise<void> {
    await copyTextToClipboard(text);
  }

  public getServerBaseUrl(): string {
    return readStoredServerBaseUrl();
  }

  public setServerBaseUrl(url: string): void {
    writeStoredServerBaseUrl(url);
  }

  public getMcpConfig(): McpConfig {
    return readStoredMcpConfig();
  }

  public setMcpConfig(mode: McpAccessMode, port: number): void {
    writeStoredMcpConfig(mode, port);
  }
}

interface OidcHttpResponsePayload {
  status: number;
  body: string;
}

interface DesktopNavigateEventPayload {
  route: string;
  params?: Record<string, string>;
}

export class TauriPlatformBridge implements PlatformBridge {
  public readonly runtime = PlatformRuntime.DESKTOP;

  private currentAccessToken: string | null = readLocalStorage(STORAGE_KEYS.ACCESS_TOKEN);
  private currentAuthMode: AuthMode | null = null;

  public async openExternalUrl(url: string): Promise<void> {
    if (!isAllowedExternalUrl(url)) {
      throw new Error('Blocked unsafe external URL: only http and https protocols are allowed');
    }
    await invoke<void>(TAURI_COMMANDS.OPEN_EXTERNAL_URL, { url: url.trim() });
  }

  public async notify(title: string, body: string): Promise<void> {
    try {
      await invoke<void>(TAURI_COMMANDS.SEND_NATIVE_NOTIFICATION, { title, body });
    } catch {
      // Ignore native notification failures in headless or restricted desktop contexts.
    }
  }

  public async copyToClipboard(text: string): Promise<void> {
    await copyTextToClipboard(text);
  }

  public getServerBaseUrl(): string {
    return readStoredServerBaseUrl();
  }

  public setServerBaseUrl(url: string): void {
    const normalized = writeStoredServerBaseUrl(url);
    void this.pushRuntimeConfig(normalized, this.currentAccessToken, this.currentAuthMode);
  }

  public getMcpConfig(): McpConfig {
    return readStoredMcpConfig();
  }

  public setMcpConfig(mode: McpAccessMode, port: number): void {
    const saved = writeStoredMcpConfig(mode, port);
    void this.pushRuntimeConfig(
      this.getServerBaseUrl(),
      this.currentAccessToken,
      this.currentAuthMode,
      saved,
    );
  }

  public async prepareOidcLoopback(): Promise<string> {
    return invoke<string>(TAURI_COMMANDS.PREPARE_OIDC_LOOPBACK);
  }

  public async openOidcSystemLogin(authorizeUrl: string): Promise<string> {
    if (!isAllowedExternalUrl(authorizeUrl)) {
      throw new Error('Blocked unsafe external URL: only http and https protocols are allowed');
    }
    const rawResult = await invoke<string>(TAURI_COMMANDS.OPEN_OIDC_SYSTEM_LOGIN, {
      authorizeUrl: authorizeUrl.trim(),
    });
    return extractOidcCallbackSearch(rawResult);
  }

  public readonly oidcFetch: typeof fetch = async (
    input: RequestInfo | URL,
    init?: RequestInit,
  ): Promise<Response> => {
    const url =
      typeof input === 'string'
        ? input
        : input instanceof URL
          ? input.toString()
          : input.url;

    if (!isAllowedExternalUrl(url)) {
      throw new Error('Blocked unsafe OIDC request URL: only http and https protocols are allowed');
    }

    const method = (init?.method ?? (input instanceof Request ? input.method : 'GET')).toUpperCase();
    const mergedHeaders = new Headers(input instanceof Request ? input.headers : undefined);
    if (init?.headers) {
      const initHeaders = new Headers(init.headers);
      initHeaders.forEach((value, key) => {
        mergedHeaders.set(key, value);
      });
    }
    const headers: Record<string, string> = {};
    mergedHeaders.forEach((value, key) => {
      headers[key] = value;
    });

    let body: string | undefined;
    if (typeof init?.body === 'string') {
      body = init.body;
    } else if (init?.body instanceof URLSearchParams) {
      body = init.body.toString();
    } else if (init?.body !== undefined && init?.body !== null) {
      body = String(init.body);
    }

    const res = await invoke<OidcHttpResponsePayload>(TAURI_COMMANDS.OIDC_HTTP_REQUEST, {
      url: url.trim(),
      method,
      headers,
      ...(body !== undefined ? { body } : {}),
    });

    const isNullBodyStatus = res.status === 204 || res.status === 205 || res.status === 304;
    return new Response(isNullBodyStatus ? null : res.body, {
      status: res.status,
      headers: {
        [HTTP_HEADERS.CONTENT_TYPE]: HTTP_HEADERS.CONTENT_TYPE_JSON,
      },
    });
  };

  public async syncRuntimeSession(session: RuntimeSessionSync): Promise<void> {
    this.currentAccessToken = session.accessToken;
    this.currentAuthMode = session.authMode;
    let normalizedUrl = '';
    try {
      normalizedUrl = normalizeBaseUrl(session.serverBaseUrl);
    } catch {
      normalizedUrl = this.getServerBaseUrl();
    }
    const mcp = this.getMcpConfig();
    await invoke<McpBridgeStatus>(TAURI_COMMANDS.SYNC_DESKTOP_RUNTIME_CONFIG, {
      serverBaseUrl: normalizedUrl,
      accessToken: session.accessToken,
      authMode: session.authMode,
      mcpMode: mcp.mode,
      mcpPort: mcp.port,
    });
  }

  public async getMcpBridgeStatus(): Promise<McpBridgeStatus> {
    return invoke<McpBridgeStatus>(TAURI_COMMANDS.GET_MCP_BRIDGE_STATUS);
  }

  public async getMcpLocalToken(): Promise<string> {
    return invoke<string>(TAURI_COMMANDS.GET_MCP_LOCAL_TOKEN);
  }

  public async rotateMcpToken(): Promise<McpBridgeStatus> {
    return invoke<McpBridgeStatus>(TAURI_COMMANDS.ROTATE_MCP_TOKEN);
  }

  public onDesktopNavigate(
    handler: (route: string, params?: Record<string, string>) => void,
  ): () => void {
    let disposed = false;
    let unlistenFn: UnlistenFn | null = null;

    void listen<DesktopNavigateEventPayload>(TAURI_EVENTS.DESKTOP_NAVIGATE, (event) => {
      if (disposed) {
        return;
      }
      const payload = event?.payload;
      if (!payload || typeof payload.route !== 'string') {
        return;
      }
      const route = payload.route.trim();
      if (!VALID_APP_ROUTES.has(route)) {
        return;
      }
      handler(route, sanitizeNavigateParams(payload.params));
    })
      .then((unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          unlistenFn = unlisten;
        }
      })
      .catch(() => {
        // Ignore listener registration failures in restricted contexts.
      });

    return () => {
      disposed = true;
      if (unlistenFn) {
        unlistenFn();
        unlistenFn = null;
      }
    };
  }

  private async pushRuntimeConfig(
    serverBaseUrl: string,
    accessToken: string | null,
    authMode: AuthMode | null,
    mcpOverride?: McpConfig,
  ): Promise<void> {
    const mcp = mcpOverride ?? this.getMcpConfig();
    try {
      await invoke<McpBridgeStatus>(TAURI_COMMANDS.SYNC_DESKTOP_RUNTIME_CONFIG, {
        serverBaseUrl,
        accessToken,
        authMode,
        mcpMode: mcp.mode,
        mcpPort: mcp.port,
      });
    } catch {
      // Fire-and-forget config sync: ignore transient IPC errors when desktop bridge is restarting.
    }
  }
}

export function createPlatformBridge(): PlatformBridge {
  if (isTauriRuntime()) {
    return new TauriPlatformBridge();
  }
  return new WebPlatformBridge();
}
