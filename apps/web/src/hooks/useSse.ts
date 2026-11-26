import { useCallback, useEffect, useRef, useState } from 'react';
import { buildApiUrl } from '../api/client';
import { API_ROUTES, DEFAULTS, HTTP_HEADERS } from '../constants/routes';
import type {
  AlertTriggeredEvent,
  DigestReadyEvent,
  HeartbeatEvent,
  RunProgressEvent,
} from '../types/api';
import { SseEventType } from '../types/enums';

export type SseConnectionStatus = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'error';

export interface ParsedSseEvent {
  event: string;
  data: string;
  id?: string;
}

export interface UseSseOptions {
  enabled: boolean;
  baseUrl: string;
  accessToken: string | null;
  onRunProgress?: (event: RunProgressEvent) => void;
  onDigestReady?: (event: DigestReadyEvent) => void;
  onAlertTriggered?: (event: AlertTriggeredEvent) => void;
  onHeartbeat?: (event: HeartbeatEvent) => void;
  fetchImpl?: typeof fetch;
}

export interface UseSseResult {
  status: SseConnectionStatus;
  lastHeartbeatAt: string | null;
  latestRunProgress: RunProgressEvent | null;
  latestAlert: AlertTriggeredEvent | null;
  clearLatestAlert: () => void;
  reconnectCount: number;
  error: string | null;
}

/**
 * Parses a single raw SSE event block (lines separated by `\n` or `\r\n`) into `{ event, data, id? }`.
 * Returns `null` for empty or comment-only blocks.
 */
export function parseSseEventBlock(rawBlock: string): ParsedSseEvent | null {
  if (!rawBlock) {
    return null;
  }

  let eventName = 'message';
  let eventId: string | undefined;
  const dataLines: string[] = [];

  const lines = rawBlock.split(/\r\n|\r|\n/);
  for (const line of lines) {
    if (!line || line.startsWith(':')) {
      continue;
    }

    const colonIndex = line.indexOf(':');
    let field: string;
    let value: string;
    if (colonIndex === -1) {
      field = line;
      value = '';
    } else {
      field = line.slice(0, colonIndex);
      const rawVal = line.slice(colonIndex + 1);
      value = rawVal.startsWith(' ') ? rawVal.slice(1) : rawVal;
    }

    if (field === 'event') {
      eventName = value || 'message';
    } else if (field === 'data') {
      dataLines.push(value);
    } else if (field === 'id' && !value.includes('\0')) {
      eventId = value;
    }
  }

  if (dataLines.length === 0) {
    return null;
  }

  const parsed: ParsedSseEvent = {
    event: eventName,
    data: dataLines.join('\n'),
  };
  if (eventId !== undefined) {
    parsed.id = eventId;
  }
  return parsed;
}

/**
 * Consumes complete SSE event blocks from a streaming text buffer and returns `{ events, remainder }`.
 */
export function consumeSseBuffer(buffer: string): {
  events: ParsedSseEvent[];
  remainder: string;
} {
  if (!buffer) {
    return { events: [], remainder: '' };
  }

  // Preserve a trailing '\r' in case '\r\n' is split across chunk boundaries.
  const hasTrailingCr = buffer.endsWith('\r');
  const working = (hasTrailingCr ? buffer.slice(0, -1) : buffer)
    .replace(/\r\n/g, '\n')
    .replace(/\r/g, '\n');

  const parts = working.split('\n\n');
  const lastPart = parts.pop() ?? '';
  const remainder = hasTrailingCr ? `${lastPart}\r` : lastPart;

  const events: ParsedSseEvent[] = [];
  for (const block of parts) {
    const parsed = parseSseEventBlock(block);
    if (parsed) {
      events.push(parsed);
    }
  }

  return { events, remainder };
}

function safeParseJson<T>(raw: string): T | null {
  try {
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

export function useSse(options: UseSseOptions): UseSseResult {
  const {
    enabled,
    baseUrl,
    accessToken,
    onRunProgress,
    onDigestReady,
    onAlertTriggered,
    onHeartbeat,
    fetchImpl,
  } = options;

  const [status, setStatus] = useState<SseConnectionStatus>('idle');
  const [lastHeartbeatAt, setLastHeartbeatAt] = useState<string | null>(null);
  const [latestRunProgress, setLatestRunProgress] = useState<RunProgressEvent | null>(null);
  const [latestAlert, setLatestAlert] = useState<AlertTriggeredEvent | null>(null);
  const [reconnectCount, setReconnectCount] = useState<number>(0);
  const [error, setError] = useState<string | null>(null);

  const onRunProgressRef = useRef(onRunProgress);
  onRunProgressRef.current = onRunProgress;
  const onDigestReadyRef = useRef(onDigestReady);
  onDigestReadyRef.current = onDigestReady;
  const onAlertTriggeredRef = useRef(onAlertTriggered);
  onAlertTriggeredRef.current = onAlertTriggered;
  const onHeartbeatRef = useRef(onHeartbeat);
  onHeartbeatRef.current = onHeartbeat;

  const clearLatestAlert = useCallback(() => {
    setLatestAlert(null);
  }, []);

  useEffect(() => {
    if (!enabled) {
      setStatus('idle');
      setError(null);
      return;
    }

    const fetcher = fetchImpl ?? globalThis.fetch;
    if (!fetcher) {
      setStatus('error');
      setError('Fetch implementation is not available');
      return;
    }

    let disposed = false;
    let attempt = 0;
    let activeController: AbortController | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

    const dispatchEvent = (evt: ParsedSseEvent) => {
      switch (evt.event) {
        case SseEventType.RUN_PROGRESS: {
          const payload = safeParseJson<RunProgressEvent>(evt.data);
          if (payload && typeof payload.runId === 'string') {
            setLatestRunProgress(payload);
            onRunProgressRef.current?.(payload);
          }
          break;
        }
        case SseEventType.DIGEST_READY: {
          const payload = safeParseJson<DigestReadyEvent>(evt.data);
          if (payload && typeof payload.digestDate === 'string') {
            onDigestReadyRef.current?.(payload);
          }
          break;
        }
        case SseEventType.ALERT_TRIGGERED: {
          const payload = safeParseJson<AlertTriggeredEvent>(evt.data);
          if (payload && Array.isArray(payload.items)) {
            setLatestAlert(payload);
            onAlertTriggeredRef.current?.(payload);
          }
          break;
        }
        case SseEventType.HEARTBEAT: {
          const payload = safeParseJson<HeartbeatEvent>(evt.data);
          const timestamp = payload?.timestamp ?? new Date().toISOString();
          setLastHeartbeatAt(timestamp);
          onHeartbeatRef.current?.({ timestamp });
          break;
        }
        default:
          break;
      }
    };

    const scheduleReconnect = (message: string) => {
      if (disposed) {
        return;
      }
      const delay = Math.min(
        DEFAULTS.SSE_RECONNECT_MAX_MS,
        DEFAULTS.SSE_RECONNECT_BASE_MS * 2 ** attempt,
      );
      attempt += 1;
      setStatus('reconnecting');
      setError(message);
      setReconnectCount((prev) => prev + 1);
      reconnectTimer = setTimeout(() => {
        if (!disposed) {
          void connect(true);
        }
      }, delay);
    };

    const connect = async (isReconnect: boolean) => {
      if (disposed) {
        return;
      }
      activeController = new AbortController();
      setStatus(isReconnect ? 'reconnecting' : 'connecting');

      const headers = new Headers();
      headers.set(HTTP_HEADERS.ACCEPT, HTTP_HEADERS.CONTENT_TYPE_TEXT_EVENT_STREAM);
      const token = accessToken?.trim();
      if (token) {
        headers.set(HTTP_HEADERS.AUTHORIZATION, `${HTTP_HEADERS.BEARER_PREFIX}${token}`);
      }

      try {
        const url = buildApiUrl(baseUrl, API_ROUTES.EVENTS_STREAM);
        const response = await fetcher(url, {
          method: 'GET',
          headers,
          signal: activeController.signal,
        });

        if (disposed) {
          return;
        }

        if (response.status === 401 || response.status === 403) {
          setStatus('error');
          setError(`SSE authentication error (${response.status})`);
          return;
        }

        if (!response.ok || !response.body) {
          scheduleReconnect(`SSE connection failed (${response.status})`);
          return;
        }

        attempt = 0;
        setStatus('connected');
        setError(null);

        const reader = response.body.getReader();
        const decoder = new TextDecoder('utf-8');
        let buffer = '';

        while (!disposed) {
          const { done, value } = await reader.read();
          if (done) {
            break;
          }
          if (value) {
            buffer += decoder.decode(value, { stream: true });
            const { events, remainder } = consumeSseBuffer(buffer);
            buffer = remainder;
            for (const evt of events) {
              dispatchEvent(evt);
            }
          }
        }

        buffer += decoder.decode();
        if (buffer.trim()) {
          const { events } = consumeSseBuffer(`${buffer}\n\n`);
          for (const evt of events) {
            dispatchEvent(evt);
          }
        }

        if (!disposed && !activeController.signal.aborted) {
          scheduleReconnect('SSE stream closed by server');
        }
      } catch (err) {
        if (disposed || activeController?.signal.aborted) {
          return;
        }
        const message = err instanceof Error ? err.message : 'SSE connection error';
        scheduleReconnect(message);
      }
    };

    void connect(false);

    return () => {
      disposed = true;
      if (reconnectTimer !== null) {
        clearTimeout(reconnectTimer);
      }
      activeController?.abort();
    };
  }, [accessToken, baseUrl, enabled, fetchImpl]);

  return {
    status,
    lastHeartbeatAt,
    latestRunProgress,
    latestAlert,
    clearLatestAlert,
    reconnectCount,
    error,
  };
}
