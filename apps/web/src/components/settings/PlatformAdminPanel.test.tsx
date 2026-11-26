import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ApiClient } from '../../api/client';
import type { PlatformSettingsView } from '../../types/api';
import { PlatformAdminPanel } from './PlatformAdminPanel';

describe('PlatformAdminPanel', () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('loads masked platform state, saves new auth_token with bridge reload feedback, and runs live X test', async () => {
    const initialView: PlatformSettingsView = {
      xBridgeBaseUrl: 'http://rsshub:1200',
      xBridgeReloadUrl: 'http://rsshub:1201/internal/twitter-token',
      hasXBridgeReloadSecret: true,
      xBridgeReloadSecretHint: 'configured',
      hasXAuthToken: false,
      xAuthTokenHint: null,
      hasXBearerToken: false,
      xBearerTokenHint: null,
      xBridgeConfigured: true,
      xIntegrationReady: true,
      updatedAt: null,
    };

    const updatedView: PlatformSettingsView = {
      ...initialView,
      hasXAuthToken: true,
      xAuthTokenHint: 'a0a3****1cac',
      updatedAt: '2026-11-26T13:00:00Z',
    };

    const api = {
      getAdminPlatformSettings: vi.fn().mockResolvedValue(initialView),
      updateAdminPlatformSettings: vi.fn().mockResolvedValue({
        settings: updatedView,
        bridgeReloaded: true,
        bridgeMessage: 'Bridge hot-reloaded with updated X auth_token',
      }),
      testAdminPlatformX: vi.fn().mockResolvedValue({
        ok: true,
        mode: 'rsshub_bridge',
        handle: 'GoogleDeepMind',
        itemsFetched: 2,
        sampleTitles: ['Gemini 2.5 post', 'AlphaFold news'],
        latencyMs: 312,
        message: 'Fetched 2 items via rsshub_bridge',
      }),
    } as unknown as ApiClient;

    render(<PlatformAdminPanel api={api} />);

    expect(await screen.findByText(/Platform X \(Twitter\) Integration/i)).toBeInTheDocument();
    expect(screen.getByText(/No auth_token Stored/i)).toBeInTheDocument();

    const tokenInput = screen.getByLabelText(/X Session auth_token/i) as HTMLInputElement;
    expect(tokenInput.value).toBe('');
    fireEvent.change(tokenInput, { target: { value: 'a0a3_sample_token_1cac' } });

    fireEvent.click(screen.getByRole('button', { name: /Save & Apply \(No Restart\)/i }));

    await waitFor(() => {
      expect(api.updateAdminPlatformSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          xBridgeBaseUrl: 'http://rsshub:1200',
          xAuthToken: 'a0a3_sample_token_1cac',
          clearXAuthToken: false,
        }),
      );
    });

    expect(await screen.findByText(/bridge hot-reloaded/i)).toBeInTheDocument();
    expect(screen.getByText(/X auth_token Saved \(a0a3\*\*\*\*1cac\)/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /Test X Fetch/i }));
    expect(await screen.findByText(/X Fetch Passed \(2 items, 312 ms, mode: rsshub_bridge\)/i)).toBeInTheDocument();
    expect(screen.getByText('Gemini 2.5 post')).toBeInTheDocument();
  });
});
