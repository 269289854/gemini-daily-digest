import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { SafeMarkdown, sanitizeHttpUrl } from '../components/SafeMarkdown';
import { DEFAULTS, HTTP_HEADERS, SSE_RUN_STAGES } from '../constants/routes';
import {
  type UseSseOptions,
  consumeSseBuffer,
  parseSseEventBlock,
  useSse,
} from '../hooks/useSse';
import type { PlatformBridge } from '../platform/bridge';
import { PlatformRuntime } from '../platform/bridge';
import { AlertPriority, McpAccessMode, RunStatus, RunType, SseEventType } from '../types/enums';

function createMockBridge(): PlatformBridge {
  return {
    runtime: PlatformRuntime.WEB,
    openExternalUrl: vi.fn().mockResolvedValue(undefined),
    notify: vi.fn().mockResolvedValue(undefined),
    copyToClipboard: vi.fn().mockResolvedValue(undefined),
    getServerBaseUrl: vi.fn().mockReturnValue(''),
    setServerBaseUrl: vi.fn(),
    getMcpConfig: vi.fn().mockReturnValue({ mode: McpAccessMode.READ_ONLY, port: 8765 }),
    setMcpConfig: vi.fn(),
  };
}

function SseHarness(props: UseSseOptions) {
  const result = useSse(props);
  return (
    <div>
      <span data-testid="status">{result.status}</span>
      <span data-testid="heartbeat">{result.lastHeartbeatAt ?? 'none'}</span>
      <span data-testid="stage">{result.latestRunProgress?.stage ?? 'none'}</span>
      <span data-testid="alert-count">{result.latestAlert?.count ?? 0}</span>
      <span data-testid="reconnects">{result.reconnectCount}</span>
      <span data-testid="error">{result.error ?? 'none'}</span>
    </div>
  );
}

describe('SSE parser, useSse hook, and SafeMarkdown', () => {
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('parses SSE event blocks including CRLF, comments, id, and multi-line data', () => {
    expect(parseSseEventBlock('')).toBeNull();
    expect(parseSseEventBlock(': keepalive comment')).toBeNull();

    const parsed = parseSseEventBlock(
      'id: evt-42\r\nevent: run_progress\r\ndata: {"line":1,\r\ndata: "ok":true}',
    );
    expect(parsed).toEqual({
      id: 'evt-42',
      event: 'run_progress',
      data: '{"line":1,\n"ok":true}',
    });

    const chunk1 = consumeSseBuffer('event: heartbeat\r\ndata: {"timestamp":"t1"}\r');
    expect(chunk1.events).toHaveLength(0);
    expect(chunk1.remainder.endsWith('\r')).toBe(true);

    const chunk2 = consumeSseBuffer(
      `${chunk1.remainder}\n\r\n: keepalive\n\nevent: digest_ready\ndata: {"digestDate":"2026-11-26"}\n\npartial`,
    );
    expect(chunk2.events).toHaveLength(2);
    expect(chunk2.events[0]).toEqual({
      event: 'heartbeat',
      data: '{"timestamp":"t1"}',
    });
    expect(chunk2.events[1]).toEqual({
      event: 'digest_ready',
      data: '{"digestDate":"2026-11-26"}',
    });
    expect(chunk2.remainder).toBe('partial');
  });

  it('streams SSE events with Bearer header, handles malformed JSON safely, and handles 401/backoff', async () => {
    const encoder = new TextEncoder();
    const onRunProgress = vi.fn();
    const onDigestReady = vi.fn();
    const onAlertTriggered = vi.fn();
    const onHeartbeat = vi.fn();

    let closeStream: (() => void) | undefined;
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        closeStream = () => controller.close();
        controller.enqueue(
          encoder.encode(
            `event: ${SseEventType.RUN_PROGRESS}\ndata: {"runId":"run-1","runType":"${RunType.MANUAL_DAILY}","status":"${RunStatus.RUNNING}","stage":"${SSE_RUN_STAGES.COLLECTING}","itemsFetched":5,"itemsNew":2}\n\n`,
          ),
        );
        controller.enqueue(encoder.encode(`event: ${SseEventType.RUN_PROGRESS}\ndata: {bad-json\n\n`));
        controller.enqueue(
          encoder.encode(
            `event: ${SseEventType.DIGEST_READY}\ndata: {"digestId":"dig-1","digestDate":"2026-11-26","title":"Daily","itemCount":5,"highlightedCount":1,"degradedFallback":false}\n\n`,
          ),
        );
        controller.enqueue(
          encoder.encode(
            `event: ${SseEventType.ALERT_TRIGGERED}\ndata: {"runId":"run-1","count":1,"items":[{"itemId":"i1","sourceId":"s1","sourceName":"Blog","title":"Gemini 2.5","canonicalUrl":"https://ai.google.dev","priority":"${AlertPriority.URGENT}","matchedKeywords":["gemini"],"publishedAt":null}]}\n\n`,
          ),
        );
        controller.enqueue(
          encoder.encode(`event: ${SseEventType.HEARTBEAT}\ndata: {"timestamp":"2026-11-26T08:00:00Z"}\n\n`),
        );
      },
    });

    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(stream, {
        status: 200,
        headers: { 'content-type': 'text/event-stream' },
      }),
    );

    render(
      <SseHarness
        enabled
        baseUrl="https://digest.example.com/"
        accessToken="test-tok-123"
        fetchImpl={fetchMock}
        onRunProgress={onRunProgress}
        onDigestReady={onDigestReady}
        onAlertTriggered={onAlertTriggered}
        onHeartbeat={onHeartbeat}
      />,
    );

    await waitFor(() => {
      expect(screen.getByTestId('status').textContent).toBe('connected');
      expect(screen.getByTestId('stage').textContent).toBe(SSE_RUN_STAGES.COLLECTING);
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const reqInit = fetchMock.mock.calls[0][1];
    const reqHeaders = reqInit?.headers as Headers;
    expect(fetchMock.mock.calls[0][0]).toBe('https://digest.example.com/api/v1/events/stream');
    expect(reqHeaders.get(HTTP_HEADERS.AUTHORIZATION)).toBe('Bearer test-tok-123');
    expect(reqHeaders.get(HTTP_HEADERS.ACCEPT)).toBe('text/event-stream');

    expect(onRunProgress).toHaveBeenCalledTimes(1);
    expect(onDigestReady).toHaveBeenCalledTimes(1);
    expect(onAlertTriggered).toHaveBeenCalledTimes(1);
    expect(onHeartbeat).toHaveBeenCalledWith({ timestamp: '2026-11-26T08:00:00Z' });
    expect(screen.getByTestId('alert-count').textContent).toBe('1');
    expect(screen.getByTestId('heartbeat').textContent).toBe('2026-11-26T08:00:00Z');

    // Close stream gracefully -> schedules reconnect
    vi.useFakeTimers();
    closeStream?.();
    await vi.waitFor(() => {
      expect(screen.getByTestId('status').textContent).toBe('reconnecting');
    });
    expect(screen.getByTestId('reconnects').textContent).toBe('1');

    // Next attempt returns 401 -> enters terminal 'error' state without further reconnect
    fetchMock.mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }));
    await vi.advanceTimersByTimeAsync(DEFAULTS.SSE_RECONNECT_BASE_MS + 50);
    await vi.waitFor(() => {
      expect(screen.getByTestId('status').textContent).toBe('error');
    });
    expect(screen.getByTestId('error').textContent).toContain('401');

    cleanup();
  });

  it('neutralizes XSS vectors in SafeMarkdown and renders GFM tables and safe links', () => {
    expect(sanitizeHttpUrl('https://ai.google.dev/a')).toBe('https://ai.google.dev/a');
    expect(sanitizeHttpUrl('javascript:alert(1)')).toBeNull();
    expect(sanitizeHttpUrl('data:text/html;base64,PHNjcmlwdD4=')).toBeNull();

    const bridge = createMockBridge();
    const markdown = [
      '# Briefing Title',
      '',
      '<script>window.__xss = true;</script>',
      '<iframe src="https://evil.example.com"></iframe>',
      '',
      '[Safe Link](https://ai.google.dev/docs) | [Bad JS](javascript:alert(1)) | [Bad Data](data:text/html,hi)',
      '',
      '| Model | Status |',
      '| --- | --- |',
      '| Gemini 2.5 | GA |',
    ].join('\n');

    const { container } = render(<SafeMarkdown content={markdown} bridge={bridge} />);

    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('iframe')).toBeNull();
    expect(container.querySelector('[dangerouslySetInnerHTML]')).toBeNull();

    const safeAnchor = screen.getByRole('link', { name: 'Safe Link' });
    expect(safeAnchor).toHaveAttribute('href', 'https://ai.google.dev/docs');
    expect(safeAnchor).toHaveAttribute('rel', 'noopener noreferrer');
    fireEvent.click(safeAnchor);
    expect(bridge.openExternalUrl).toHaveBeenCalledWith('https://ai.google.dev/docs');

    const blockedLinks = container.querySelectorAll('[data-blocked-link="true"]');
    expect(blockedLinks.length).toBe(2);
    expect(screen.getByText('Bad JS').closest('a')).toBeNull();
    expect(screen.getByText('Bad Data').closest('a')).toBeNull();

    expect(container.querySelector('table')).not.toBeNull();
    expect(screen.getByText('Gemini 2.5')).toBeInTheDocument();
  });
});
