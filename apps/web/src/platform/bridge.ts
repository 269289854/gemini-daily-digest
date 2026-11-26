/**
 * Adapter pattern isolating Web Browser vs Tauri 2 Desktop runtime differences.
 * UI pages never inspect `window.__TAURI__` directly.
 */

import { normalizeBaseUrl } from '../api/client';
import { DEFAULTS, STORAGE_KEYS } from '../constants/routes';
import { McpAccessMode } from '../types/enums';

export const PlatformRuntime = {
  WEB: 'web',
  DESKTOP: 'desktop',
} as const;
export type PlatformRuntime = (typeof PlatformRuntime)[keyof typeof PlatformRuntime];

export interface McpConfig {
  mode: McpAccessMode;
  port: number;
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
  openOidcSystemLogin?(authorizeUrl: string): Promise<string>;
  onDesktopNavigate?(handler: (route: string, params?: Record<string, string>) => void): () => void;
}

const VALID_MCP_MODES: ReadonlySet<string> = new Set(Object.values(McpAccessMode));

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

  public getServerBaseUrl(): string {
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

  public setServerBaseUrl(url: string): void {
    const normalized = normalizeBaseUrl(url);
    writeLocalStorage(STORAGE_KEYS.SERVER_BASE_URL, normalized ? normalized : null);
  }

  public getMcpConfig(): McpConfig {
    const mode = normalizeMcpMode(readLocalStorage(STORAGE_KEYS.MCP_ACCESS_MODE));
    const port = normalizeMcpPort(readLocalStorage(STORAGE_KEYS.MCP_PORT));
    return { mode, port };
  }

  public setMcpConfig(mode: McpAccessMode, port: number): void {
    const safeMode = VALID_MCP_MODES.has(mode) ? mode : McpAccessMode.READ_ONLY;
    const safePort = normalizeMcpPort(port);
    writeLocalStorage(STORAGE_KEYS.MCP_ACCESS_MODE, safeMode);
    writeLocalStorage(STORAGE_KEYS.MCP_PORT, String(safePort));
  }
}

export function createPlatformBridge(): PlatformBridge {
  return new WebPlatformBridge();
}
