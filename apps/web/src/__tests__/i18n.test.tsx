import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from '../App';
import { STORAGE_KEYS } from '../constants/routes';
import { resolveInitialLocale, translateMessage } from '../i18n';
import { enMessages } from '../i18n/locales/en';
import { zhMessages } from '../i18n/locales/zh';
import { PlatformRuntime, type PlatformBridge } from '../platform/bridge';
import { AuthMode, Locale, McpAccessMode } from '../types/enums';

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function createMockBridge(): PlatformBridge {
  return {
    runtime: PlatformRuntime.WEB,
    openExternalUrl: vi.fn().mockResolvedValue(undefined),
    notify: vi.fn().mockResolvedValue(undefined),
    copyToClipboard: vi.fn().mockResolvedValue(undefined),
    getServerBaseUrl: vi.fn().mockReturnValue(''),
    setServerBaseUrl: vi.fn(),
    getMcpConfig: vi.fn().mockReturnValue({
      mode: McpAccessMode.READ_ONLY,
      port: 8765,
    }),
    setMcpConfig: vi.fn(),
  };
}

describe('Bilingual i18n dictionaries, detection, and live switching', () => {
  const originalNavigatorLanguage = Object.getOwnPropertyDescriptor(
    window.navigator,
    'language',
  );
  const originalNavigatorLanguages = Object.getOwnPropertyDescriptor(
    window.navigator,
    'languages',
  );

  beforeEach(() => {
    window.localStorage.clear();
    window.sessionStorage.clear();
    window.history.replaceState(null, '', '/');
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    if (originalNavigatorLanguage) {
      Object.defineProperty(window.navigator, 'language', originalNavigatorLanguage);
    }
    if (originalNavigatorLanguages) {
      Object.defineProperty(window.navigator, 'languages', originalNavigatorLanguages);
    }
  });

  it('maintains 1:1 key parity and placeholder parity between enMessages and zhMessages', () => {
    const enKeys = Object.keys(enMessages).sort();
    const zhKeys = Object.keys(zhMessages).sort();
    expect(zhKeys).toEqual(enKeys);

    const placeholderRegex = /\{[A-Za-z0-9_]+\}/g;
    for (const key of enKeys) {
      const k = key as keyof typeof enMessages;
      const enVal = enMessages[k];
      const zhVal = zhMessages[k];
      expect(enVal.trim().length, `empty en value for ${key}`).toBeGreaterThan(0);
      expect(zhVal.trim().length, `empty zh value for ${key}`).toBeGreaterThan(0);

      const enPlaceholders = (enVal.match(placeholderRegex) ?? []).sort();
      const zhPlaceholders = (zhVal.match(placeholderRegex) ?? []).sort();
      expect(zhPlaceholders, `placeholder mismatch for ${key}`).toEqual(enPlaceholders);
    }
  });

  it('resolves initial locale from localStorage first, then browser language', () => {
    Object.defineProperty(window.navigator, 'languages', {
      configurable: true,
      value: ['zh-CN', 'en-US'],
    });
    expect(resolveInitialLocale()).toBe(Locale.ZH_CN);

    Object.defineProperty(window.navigator, 'languages', {
      configurable: true,
      value: ['zh-TW'],
    });
    expect(resolveInitialLocale()).toBe(Locale.ZH_CN);

    Object.defineProperty(window.navigator, 'languages', {
      configurable: true,
      value: ['en-US'],
    });
    expect(resolveInitialLocale()).toBe(Locale.EN_US);

    window.localStorage.setItem(STORAGE_KEYS.LOCALE, Locale.ZH_CN);
    expect(resolveInitialLocale()).toBe(Locale.ZH_CN);
  });

  it('interpolates parameters cleanly', () => {
    expect(translateMessage(Locale.ZH_CN, 'ai.modelsLoadedStatus', { count: 5, ms: 42 })).toContain(
      '5 个模型（42 ms）',
    );
    expect(translateMessage(Locale.EN_US, 'ai.modelsLoadedStatus', { count: 5, ms: 42 })).toContain(
      'Loaded 5 models (42 ms)',
    );
  });

  it('toggles language live in App, persists to localStorage, and updates document.documentElement.lang', async () => {
    Object.defineProperty(window.navigator, 'languages', {
      configurable: true,
      value: ['en-US'],
    });

    const fetchMock = vi.fn().mockImplementation(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith('/api/v1/auth/config')) {
        return jsonResponse({
          authMode: AuthMode.OIDC,
          issuerUrl: 'https://auth.example.com/application/o/digest',
          clientId: 'digest-web',
          scopes: ['openid', 'profile', 'email'],
        });
      }
      return jsonResponse({ code: 'UNAUTHORIZED', message: 'Not signed in', requestId: 'r1' }, 401);
    });

    render(<App bridge={createMockBridge()} fetchImpl={fetchMock as unknown as typeof fetch} />);

    expect(await screen.findByRole('button', { name: /Sign in with OIDC/i })).toBeInTheDocument();
    expect(document.documentElement.lang).toBe(Locale.EN_US);

    const zhButton = screen.getByRole('button', { name: '中文' });
    fireEvent.click(zhButton);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /通过 OIDC 单点登录/i })).toBeInTheDocument();
    });
    expect(document.documentElement.lang).toBe(Locale.ZH_CN);
    expect(window.localStorage.getItem(STORAGE_KEYS.LOCALE)).toBe(Locale.ZH_CN);

    const enButton = screen.getByRole('button', { name: 'EN' });
    fireEvent.click(enButton);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /Sign in with OIDC/i })).toBeInTheDocument();
    });
    expect(document.documentElement.lang).toBe(Locale.EN_US);
    expect(window.localStorage.getItem(STORAGE_KEYS.LOCALE)).toBe(Locale.EN_US);
  });
});
