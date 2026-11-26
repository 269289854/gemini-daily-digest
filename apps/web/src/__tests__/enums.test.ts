import { describe, expect, it } from 'vitest';
import { API_ROUTES, APP_ROUTES, STORAGE_KEYS } from '../constants/routes';
import { PlatformRuntime, createPlatformBridge } from '../platform/bridge';
import {
  AlertPriority,
  AuthMode,
  DeliveryStatus,
  ErrorCode,
  FetchOutcome,
  McpAccessMode,
  McpToolName,
  NotificationChannelType,
  RunStatus,
  RunType,
  SourceScope,
  SourceType,
  SseEventType,
} from '../types/enums';

describe('Round 0 shared enums, routes, and platform bridge', () => {
  it('exposes complete enum sets without empty strings', () => {
    const groups = [
      SourceType,
      SourceScope,
      FetchOutcome,
      NotificationChannelType,
      RunType,
      RunStatus,
      DeliveryStatus,
      AlertPriority,
      McpAccessMode,
      AuthMode,
      SseEventType,
      ErrorCode,
      McpToolName,
    ];
    for (const group of groups) {
      const values = Object.values(group);
      expect(values.length).toBeGreaterThan(0);
      for (const val of values) {
        expect(val.trim().length).toBeGreaterThan(0);
      }
    }
  });

  it('builds URL-encoded dynamic API routes', () => {
    expect(API_ROUTES.HEALTH).toBe('/health');
    expect(API_ROUTES.digestByDate('2026-11-26')).toBe('/api/v1/digests/2026-11-26');
    expect(API_ROUTES.sourceDigests('id/with space')).toBe('/api/v1/sources/id%2Fwith%20space/digests');
    expect(APP_ROUTES.DAILY).toBe('/daily');
    expect(STORAGE_KEYS.SERVER_BASE_URL).toContain('gemini_digest');
  });

  it('creates WebPlatformBridge by default', () => {
    const bridge = createPlatformBridge();
    expect(bridge.runtime).toBe(PlatformRuntime.WEB);
  });
});
