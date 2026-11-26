import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, type ApiClient } from '../api/client';
import { SSE_RUN_STAGES } from '../constants/routes';
import { DailyDigestPage } from '../pages/DailyDigestPage';
import { HistorySearchPage } from '../pages/HistorySearchPage';
import { RunsStatsPage } from '../pages/RunsStatsPage';
import { SourcesExplorerPage } from '../pages/SourcesExplorerPage';
import type { PlatformBridge } from '../platform/bridge';
import { PlatformRuntime } from '../platform/bridge';
import type {
  DailyDigestDetail,
  DeliveryLogRecord,
  DigestRunView,
  ItemView,
  RunStatsView,
  UserSourceView,
} from '../types/api';
import {
  AlertPriority,
  DeliveryStatus,
  ErrorCode,
  FetchOutcome,
  McpAccessMode,
  NotificationChannelType,
  RunStatus,
  RunType,
  SourceScope,
  SourceType,
} from '../types/enums';

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

function makeSampleSource(overrides: Partial<UserSourceView> = {}): UserSourceView {
  return {
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
    lastFetchedAt: '2026-11-26T08:00:00Z',
    nextPollAt: '2026-11-26T09:00:00Z',
    lastStatus: FetchOutcome.UPDATED,
    lastError: null,
    consecutiveFailures: 0,
    createdAt: '2026-11-20T00:00:00Z',
    ...overrides,
  };
}

function makeSampleItem(overrides: Partial<ItemView> = {}): ItemView {
  return {
    id: 'item-1',
    sourceId: 'src-1',
    sourceName: 'Google AI Blog',
    category: 'Official',
    externalId: 'ext-1',
    canonicalUrl: 'https://ai.google.dev/updates/gemini-2-5',
    title: 'Gemini 2.5 Pro Release Notes',
    author: 'Gemini Team',
    summary: 'New reasoning capabilities and longer context window.',
    bodyPruned: false,
    publishedAt: '2026-11-26T07:30:00Z',
    fetchedAt: '2026-11-26T08:00:00Z',
    matchedKeywords: ['gemini 2.5'],
    priority: AlertPriority.URGENT,
    importanceScore: 95,
    bookmarked: false,
    bookmarkNote: null,
    ...overrides,
  };
}

function makeSampleDigest(overrides: Partial<DailyDigestDetail> = {}): DailyDigestDetail {
  return {
    id: 'dig-1',
    digestDate: '2026-11-26',
    title: 'Gemini Ecosystem Daily Digest',
    itemCount: 2,
    highlightedCount: 1,
    promptTokens: 1200,
    completionTokens: 350,
    modelUsed: 'deterministic-fallback',
    degradedFallback: true,
    createdAt: '2026-11-26T08:30:00Z',
    inputFingerprint: 'fp-1',
    overviewMarkdown: '## Executive Summary\n\nMajor **Gemini 2.5** release today.',
    overviewHtml: '',
    sections: [
      {
        category: 'Official',
        sourceId: 'src-1',
        sourceName: 'Google AI Blog',
        summaryMarkdown: 'Official announcement summarizing *Gemini 2.5* updates.',
        keyPoints: ['Reasoning boost', '1M context'],
        items: [
          {
            externalId: 'ext-1',
            canonicalUrl: 'https://ai.google.dev/updates/gemini-2-5',
            contentHash: 'hash-1',
            title: 'Gemini 2.5 Pro Release Notes',
            author: 'Gemini Team',
            summary: 'New reasoning capabilities.',
            publishedAt: '2026-11-26T07:30:00Z',
            sourceId: 'src-1',
            sourceName: 'Google AI Blog',
            category: 'Official',
            matchResult: {
              matchedKeywords: ['gemini 2.5'],
              priority: AlertPriority.URGENT,
              importanceScore: 95,
              notifyImmediately: true,
            },
          },
        ],
      },
    ],
    ...overrides,
  };
}

describe('Round 3 Page Components', () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('DailyDigestPage renders urgent banner, degradedFallback badge, Markdown copy, bookmark toggle, and SSE progress', async () => {
    const bridge = createMockBridge();
    const digestToday = makeSampleDigest();
    const digestYesterday = makeSampleDigest({
      id: 'dig-0',
      digestDate: '2026-11-25',
      degradedFallback: false,
      modelUsed: 'deepseek-chat',
      title: 'Yesterday Digest',
    });
    const itemRecord = makeSampleItem();

    const api = {
      listDigests: vi.fn().mockResolvedValue({
        items: [digestToday, digestYesterday],
        page: 1,
        pageSize: 20,
        total: 2,
        totalPages: 1,
      }),
      getDigest: vi.fn(async (date: string) =>
        date === '2026-11-25' ? digestYesterday : digestToday,
      ),
      getDigestMarkdown: vi.fn().mockResolvedValue('# Gemini Ecosystem Daily Digest\n\nMarkdown body'),
      searchItems: vi.fn().mockResolvedValue({
        items: [itemRecord],
        page: 1,
        pageSize: 100,
        total: 1,
        totalPages: 1,
      }),
      upsertBookmark: vi.fn().mockResolvedValue({
        id: 'bm-1',
        itemId: itemRecord.id,
        note: null,
        createdAt: '2026-11-26T09:00:00Z',
      }),
      deleteBookmark: vi.fn().mockResolvedValue(undefined),
      triggerRun: vi.fn().mockResolvedValue({
        id: 'run-2',
        runType: RunType.MANUAL_DAILY,
        status: RunStatus.SUCCEEDED,
        startedAt: '2026-11-26T09:01:00Z',
        finishedAt: '2026-11-26T09:01:10Z',
        itemsFetched: 4,
        itemsNew: 2,
        promptTokens: 100,
        completionTokens: 50,
        errorSummary: null,
        deliveries: [],
      }),
    } as unknown as ApiClient;

    const onNavigate = vi.fn();
    render(
      <DailyDigestPage
        api={api}
        bridge={bridge}
        onNavigate={onNavigate}
        latestRunProgress={{
          runId: 'run-live-1234',
          runType: RunType.MANUAL_DAILY,
          status: RunStatus.RUNNING,
          stage: SSE_RUN_STAGES.SUMMARIZING,
          itemsFetched: 8,
          itemsNew: 3,
          message: 'Summarizing Official sources...',
        }}
      />,
    );

    expect(await screen.findByText('Deterministic Fallback Summarization Active')).toBeInTheDocument();
    expect(screen.getByText(/Watchlist Priority Alerts \(1\)/)).toBeInTheDocument();
    expect(screen.getByText(/Summarizing Official sources/)).toBeInTheDocument();
    expect(screen.getByText('Executive Summary')).toBeInTheDocument();

    // Safe external link click
    fireEvent.click(screen.getByRole('link', { name: /Gemini 2\.5 Pro Release Notes/i }));
    expect(bridge.openExternalUrl).toHaveBeenCalledWith('https://ai.google.dev/updates/gemini-2-5');

    // Copy Markdown
    fireEvent.click(screen.getByRole('button', { name: /Copy Markdown/i }));
    await waitFor(() => {
      expect(api.getDigestMarkdown).toHaveBeenCalledWith('2026-11-26');
      expect(bridge.copyToClipboard).toHaveBeenCalledWith(
        '# Gemini Ecosystem Daily Digest\n\nMarkdown body',
      );
    });

    // Bookmark toggle -> add then remove
    fireEvent.click(screen.getByRole('button', { name: 'Bookmark item' }));
    await waitFor(() => {
      expect(api.upsertBookmark).toHaveBeenCalledWith({ itemId: 'item-1', note: undefined });
    });
    const removeBtn = await screen.findByRole('button', { name: 'Remove bookmark' });
    fireEvent.click(removeBtn);
    await waitFor(() => {
      expect(api.deleteBookmark).toHaveBeenCalledWith('item-1');
    });

    // Trigger Run
    fireEvent.click(screen.getByRole('button', { name: /Run Now/i }));
    await waitFor(() => {
      expect(api.triggerRun).toHaveBeenCalledWith({
        digestDate: '2026-11-26',
        forceRefresh: false,
        notify: false,
      });
    });

    // Date switch via pill
    fireEvent.click(screen.getByRole('button', { name: '2026-11-25' }));
    await waitFor(() => {
      expect(screen.getByText('Yesterday Digest')).toBeInTheDocument();
    });

    // 404 empty state check
    vi.mocked(api.getDigest).mockRejectedValueOnce(
      new ApiError('Not found', 404, ErrorCode.NOT_FOUND, 'req-404'),
    );
    fireEvent.change(screen.getByLabelText('Digest Date'), { target: { value: '2026-11-20' } });
    expect(await screen.findByText(/No digest generated for 2026-11-20/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Trigger First Run/i })).toBeInTheDocument();
  });

  it('SourcesExplorerPage displays health status, per-source daily summaries, and pruned raw items', async () => {
    const bridge = createMockBridge();
    const src1 = makeSampleSource();
    const src2 = makeSampleSource({
      id: 'src-2',
      customName: 'X Research Account',
      sourceType: SourceType.X_ACCOUNT,
      scope: SourceScope.USER_PRIVATE,
      lastStatus: FetchOutcome.FAILED,
      consecutiveFailures: 3,
      lastError: '429 Too Many Requests',
    });

    const api = {
      listSources: vi.fn().mockResolvedValue([src1, src2]),
      getSourceDigests: vi.fn().mockResolvedValue({
        items: [
          {
            id: 'sd-1',
            digestId: 'dig-1',
            sourceId: 'src-1',
            sourceName: 'Google AI Blog',
            digestDate: '2026-11-26',
            inputFingerprint: 'fp-sd-1',
            summaryMarkdown: 'Daily independent summary for Google AI Blog.',
            keyPoints: ['Point A'],
            itemCount: 1,
            createdAt: '2026-11-26T08:00:00Z',
          },
        ],
        page: 1,
        pageSize: 20,
        total: 1,
        totalPages: 1,
      }),
      getSourceItems: vi.fn().mockResolvedValue({
        items: [
          makeSampleItem({
            id: 'item-pruned',
            title: 'Archived 2025 Announcement',
            summary: null,
            bodyPruned: true,
          }),
        ],
        page: 1,
        pageSize: 20,
        total: 1,
        totalPages: 1,
      }),
      upsertBookmark: vi.fn().mockResolvedValue({
        id: 'bm-2',
        itemId: 'item-pruned',
        note: null,
        createdAt: '2026-11-26T09:00:00Z',
      }),
      deleteBookmark: vi.fn().mockResolvedValue(undefined),
    } as unknown as ApiClient;

    render(<SourcesExplorerPage api={api} bridge={bridge} />);

    expect(await screen.findByText('3 failures')).toBeInTheDocument();
    expect(screen.getByText('429 Too Many Requests')).toBeInTheDocument();
    expect(
      await screen.findByText('Daily independent summary for Google AI Blog.'),
    ).toBeInTheDocument();
    expect(await screen.findByText('Body pruned (>180 days)')).toBeInTheDocument();

    // Switch to failing source
    fireEvent.click(screen.getByRole('button', { name: /X Research Account/i }));
    await waitFor(() => {
      expect(api.getSourceDigests).toHaveBeenCalledWith('src-2', { page: 1, pageSize: 20 });
      expect(api.getSourceItems).toHaveBeenCalledWith('src-2', { page: 1, pageSize: 20 });
    });
  });

  it('HistorySearchPage validates dateFrom > dateTo and saves inline bookmark notes', async () => {
    const bridge = createMockBridge();
    const item = makeSampleItem({ id: 'item-hist-1', bookmarked: false, bookmarkNote: null });
    const api = {
      listSources: vi.fn().mockResolvedValue([makeSampleSource()]),
      searchItems: vi.fn().mockResolvedValue({
        items: [item],
        page: 1,
        pageSize: 20,
        total: 1,
        totalPages: 1,
      }),
      upsertBookmark: vi.fn().mockResolvedValue({
        id: 'bm-99',
        itemId: 'item-hist-1',
        note: 'Important benchmark update',
        createdAt: '2026-11-26T10:00:00Z',
      }),
      deleteBookmark: vi.fn().mockResolvedValue(undefined),
    } as unknown as ApiClient;

    render(<HistorySearchPage api={api} bridge={bridge} />);

    expect(await screen.findByText('Gemini 2.5 Pro Release Notes')).toBeInTheDocument();

    // Invalid date range blocks search
    fireEvent.change(screen.getByLabelText('Date from'), { target: { value: '2026-11-28' } });
    fireEvent.change(screen.getByLabelText('Date to'), { target: { value: '2026-11-20' } });
    expect(screen.getByRole('alert')).toHaveTextContent(/Start date must be on or before end date/i);
    const searchCallsBefore = vi.mocked(api.searchItems).mock.calls.length;
    fireEvent.click(screen.getByRole('button', { name: /^Search$/i }));
    expect(vi.mocked(api.searchItems).mock.calls.length).toBe(searchCallsBefore);

    // Fix date range and search with query
    fireEvent.change(screen.getByLabelText('Date to'), { target: { value: '2026-11-29' } });
    fireEvent.change(screen.getByLabelText('Search query'), { target: { value: 'Gemini' } });
    fireEvent.click(screen.getByRole('button', { name: /^Search$/i }));
    await waitFor(() => {
      expect(api.searchItems).toHaveBeenLastCalledWith(
        expect.objectContaining({
          q: 'Gemini',
          dateFrom: '2026-11-28',
          dateTo: '2026-11-29',
          page: 1,
        }),
      );
    });

    // Inline note edit & save
    fireEvent.click(screen.getByRole('button', { name: /Add Note/i }));
    fireEvent.change(screen.getByPlaceholderText(/Write a personal note/i), {
      target: { value: 'Important benchmark update' },
    });
    fireEvent.click(screen.getByRole('button', { name: /Save Note/i }));
    await waitFor(() => {
      expect(api.upsertBookmark).toHaveBeenCalledWith({
        itemId: 'item-hist-1',
        note: 'Important benchmark update',
      });
    });
    expect(await screen.findByText('Important benchmark update')).toBeInTheDocument();
  });

  it('RunsStatsPage renders KPIs, token bars, source health, and enforces Resend non-null digestId+channelId', async () => {
    const stats: RunStatsView = {
      totalRuns: 10,
      succeededRuns: 8,
      failedRuns: 2,
      totalPromptTokens: 4000,
      totalCompletionTokens: 1000,
      deliveriesSent: 9,
      deliveriesFailed: 1,
      sourceHealth: [
        {
          sourceId: 'src-1',
          sourceName: 'Google AI Blog',
          sourceType: SourceType.RSS,
          scope: SourceScope.SHARED_PUBLIC,
          enabled: true,
          lastStatus: FetchOutcome.UPDATED,
          lastFetchedAt: '2026-11-26T08:00:00Z',
          consecutiveFailures: 0,
          lastError: null,
        },
      ],
    };

    const resendableDelivery: DeliveryLogRecord = {
      id: 'del-ok',
      runId: 'run-1',
      userId: 'u1',
      digestId: 'dig-1',
      channelId: 'ch-1',
      channelType: NotificationChannelType.TELEGRAM,
      status: DeliveryStatus.FAILED,
      errorMessage: 'Timeout',
      sentAt: '2026-11-26T08:31:00Z',
    };
    const orphanedDelivery: DeliveryLogRecord = {
      id: 'del-null',
      runId: 'run-1',
      userId: 'u1',
      digestId: null,
      channelId: 'ch-1',
      channelType: NotificationChannelType.SMTP,
      status: DeliveryStatus.FAILED,
      errorMessage: 'No digest',
      sentAt: '2026-11-26T08:32:00Z',
    };

    const run1: DigestRunView = {
      id: 'run-1',
      runType: RunType.SCHEDULED_DAILY,
      status: RunStatus.PARTIAL,
      startedAt: '2026-11-26T08:30:00Z',
      finishedAt: '2026-11-26T08:31:00Z',
      itemsFetched: 25,
      itemsNew: 5,
      promptTokens: 4000,
      completionTokens: 1000,
      errorSummary: null,
      deliveries: [resendableDelivery, orphanedDelivery],
    };

    const api = {
      getRunStats: vi.fn().mockResolvedValue(stats),
      listRuns: vi.fn().mockResolvedValue({
        items: [run1],
        page: 1,
        pageSize: 20,
        total: 1,
        totalPages: 1,
      }),
      resendDelivery: vi.fn().mockResolvedValue({
        ...resendableDelivery,
        status: DeliveryStatus.SENT,
        errorMessage: null,
      }),
    } as unknown as ApiClient;

    render(<RunsStatsPage api={api} />);

    expect(await screen.findByText('90%')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: /Token share: 80% prompt, 20% completion/i })).toBeInTheDocument();
    expect(screen.getByRole('img', { name: /Stacked prompt and completion tokens/i })).toBeInTheDocument();
    expect(screen.getByText('Google AI Blog')).toBeInTheDocument();

    const resendButtons = screen.getAllByRole('button', { name: /Resend/i });
    expect(resendButtons).toHaveLength(2);
    expect(resendButtons[0]).toBeEnabled();
    expect(resendButtons[1]).toBeDisabled();

    fireEvent.click(resendButtons[0]);
    await waitFor(() => {
      expect(api.resendDelivery).toHaveBeenCalledWith('del-ok');
    });
    expect(await screen.findByText(/Delivery del-ok status: sent/i)).toBeInTheDocument();
  });
});
