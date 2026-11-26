import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  Bookmark,
  BookmarkCheck,
  Calendar,
  Check,
  Copy,
  ExternalLink,
  Flame,
  Loader2,
  Play,
  RefreshCw,
  Sparkles,
} from 'lucide-react';
import { ApiError, type ApiClient } from '../api/client';
import { SafeMarkdown, sanitizeHttpUrl } from '../components/SafeMarkdown';
import { APP_ROUTES, DEFAULTS, SSE_RUN_STAGES, type SseRunStage } from '../constants/routes';
import { type TranslateFn, type TranslationKey, useI18n } from '../i18n';
import type { PlatformBridge } from '../platform/bridge';
import type {
  DailyDigestDetail,
  DailyDigestSummary,
  DigestReadyEvent,
  ItemView,
  ProcessedItem,
  RunProgressEvent,
} from '../types/api';
import { AlertPriority, ErrorCode, RunStatus } from '../types/enums';

export interface DailyDigestPageProps {
  api: ApiClient;
  bridge: PlatformBridge;
  latestRunProgress?: RunProgressEvent | null;
  latestDigestReady?: DigestReadyEvent | null;
  initialDate?: string;
  onNavigate?: (route: string, params?: Record<string, string>) => void;
}

interface UiErrorState {
  code?: ErrorCode;
  message: string;
  requestId?: string;
}

const RUN_STAGE_ORDER: readonly SseRunStage[] = [
  SSE_RUN_STAGES.QUEUED,
  SSE_RUN_STAGES.COLLECTING,
  SSE_RUN_STAGES.MATCHING,
  SSE_RUN_STAGES.SUMMARIZING,
  SSE_RUN_STAGES.RENDERING,
  SSE_RUN_STAGES.NOTIFYING,
  SSE_RUN_STAGES.COMPLETED,
];

function formatLocalToday(): string {
  const now = new Date();
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, '0');
  const d = String(now.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

function toUiError(err: unknown, t: TranslateFn): UiErrorState {
  if (err instanceof ApiError) {
    return {
      code: err.code,
      message: err.message,
      requestId: err.requestId,
    };
  }
  if (err instanceof Error) {
    return { message: err.message };
  }
  return { message: t('common.unexpectedError') };
}

function stageTranslationKey(stage: string): TranslationKey | null {
  if ((RUN_STAGE_ORDER as readonly string[]).includes(stage)) {
    return `sse.stage.${stage as SseRunStage}` as TranslationKey;
  }
  return null;
}

function priorityBadgeClass(priority: AlertPriority): string {
  switch (priority) {
    case AlertPriority.URGENT:
      return 'border-rose-500/50 bg-rose-500/15 text-rose-300';
    case AlertPriority.HIGH:
      return 'border-amber-500/50 bg-amber-500/15 text-amber-300';
    default:
      return 'border-slate-700 bg-slate-800/80 text-slate-300';
  }
}

export function DailyDigestPage({
  api,
  bridge,
  latestRunProgress,
  latestDigestReady,
  initialDate,
  onNavigate,
}: DailyDigestPageProps) {
  const { t, formatDateTime, labelAlertPriority, labelRunStatus } = useI18n();
  const tRef = useRef(t);
  tRef.current = t;

  const [availableDigests, setAvailableDigests] = useState<DailyDigestSummary[]>([]);
  const [selectedDate, setSelectedDate] = useState<string>(initialDate ?? '');
  const [digest, setDigest] = useState<DailyDigestDetail | null>(null);
  const [notFoundForDate, setNotFoundForDate] = useState(false);

  const [loadingDates, setLoadingDates] = useState(true);
  const [loadingDigest, setLoadingDigest] = useState(false);
  const [error, setError] = useState<UiErrorState | null>(null);

  const [forceRefresh, setForceRefresh] = useState(false);
  const [notifyOnRun, setNotifyOnRun] = useState(false);
  const [triggeringRun, setTriggeringRun] = useState(false);

  const [copyStatus, setCopyStatus] = useState<'idle' | 'copying' | 'copied'>('idle');
  const [itemsByUrl, setItemsByUrl] = useState<Record<string, ItemView>>({});
  const [bookmarkBusyUrl, setBookmarkBusyUrl] = useState<string | null>(null);

  const copyTimerRef = useRef<number | null>(null);
  const lastDigestReadyKeyRef = useRef<string | null>(null);

  useEffect(() => {
    return () => {
      if (copyTimerRef.current !== null) {
        window.clearTimeout(copyTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (initialDate) {
      setSelectedDate(initialDate);
    }
  }, [initialDate]);

  const loadAvailableDates = useCallback(async () => {
    setLoadingDates(true);
    try {
      const paged = await api.listDigests({ page: 1, pageSize: DEFAULTS.PAGE_SIZE });
      setAvailableDigests(paged.items);
      setSelectedDate((current) => {
        if (current) {
          return current;
        }
        if (paged.items.length > 0) {
          return paged.items[0].digestDate;
        }
        return formatLocalToday();
      });
    } catch (err) {
      setError(toUiError(err, tRef.current));
      setSelectedDate((current) => current || formatLocalToday());
    } finally {
      setLoadingDates(false);
    }
  }, [api]);

  const loadDigestItemsIndex = useCallback(
    async (digestDate: string) => {
      try {
        const result = await api.searchItems({
          dateFrom: digestDate,
          dateTo: digestDate,
          page: 1,
          pageSize: DEFAULTS.MAX_PAGE_SIZE,
        });
        const nextMap: Record<string, ItemView> = {};
        for (const item of result.items) {
          nextMap[item.canonicalUrl] = item;
        }
        setItemsByUrl(nextMap);
      } catch {
        // Non-fatal: bookmark resolution falls back to on-demand lookup if clicked.
      }
    },
    [api],
  );

  const loadDigest = useCallback(
    async (targetDate: string) => {
      if (!targetDate) {
        return;
      }
      setLoadingDigest(true);
      setError(null);
      setNotFoundForDate(false);
      try {
        const detail = await api.getDigest(targetDate);
        setDigest(detail);
        void loadDigestItemsIndex(detail.digestDate);
      } catch (err) {
        if (err instanceof ApiError && (err.status === 404 || err.code === ErrorCode.NOT_FOUND)) {
          setDigest(null);
          setItemsByUrl({});
          setNotFoundForDate(true);
        } else {
          setError(toUiError(err, tRef.current));
        }
      } finally {
        setLoadingDigest(false);
      }
    },
    [api, loadDigestItemsIndex],
  );

  useEffect(() => {
    void loadAvailableDates();
  }, [loadAvailableDates]);

  useEffect(() => {
    if (selectedDate) {
      void loadDigest(selectedDate);
    }
  }, [selectedDate, loadDigest]);

  useEffect(() => {
    if (!latestDigestReady) {
      return;
    }
    const eventKey = `${latestDigestReady.digestId}:${latestDigestReady.digestDate}`;
    if (lastDigestReadyKeyRef.current === eventKey) {
      return;
    }
    lastDigestReadyKeyRef.current = eventKey;

    void loadAvailableDates();
    if (!selectedDate || selectedDate === latestDigestReady.digestDate || notFoundForDate) {
      if (selectedDate !== latestDigestReady.digestDate) {
        setSelectedDate(latestDigestReady.digestDate);
      } else {
        void loadDigest(latestDigestReady.digestDate);
      }
    }
  }, [latestDigestReady, selectedDate, notFoundForDate, loadAvailableDates, loadDigest]);

  const handleTriggerRun = async () => {
    setTriggeringRun(true);
    setError(null);
    try {
      await api.triggerRun({
        digestDate: selectedDate || undefined,
        forceRefresh,
        notify: notifyOnRun,
      });
      await loadAvailableDates();
      if (selectedDate) {
        await loadDigest(selectedDate);
      }
    } catch (err) {
      setError(toUiError(err, t));
    } finally {
      setTriggeringRun(false);
    }
  };

  const handleCopyMarkdown = async () => {
    if (!digest) {
      return;
    }
    setCopyStatus('copying');
    setError(null);
    try {
      const markdown = await api.getDigestMarkdown(digest.digestDate);
      await bridge.copyToClipboard(markdown);
      setCopyStatus('copied');
      if (copyTimerRef.current !== null) {
        window.clearTimeout(copyTimerRef.current);
      }
      copyTimerRef.current = window.setTimeout(() => {
        setCopyStatus('idle');
      }, 2500);
    } catch (err) {
      setCopyStatus('idle');
      setError(toUiError(err, t));
    }
  };

  const handleOpenExternal = (rawUrl: string) => {
    const safe = sanitizeHttpUrl(rawUrl);
    if (!safe) {
      return;
    }
    void bridge.openExternalUrl(safe);
  };

  const handleToggleBookmark = async (item: ProcessedItem) => {
    setBookmarkBusyUrl(item.canonicalUrl);
    setError(null);
    try {
      let resolved: ItemView | undefined = itemsByUrl[item.canonicalUrl];
      if (!resolved) {
        const searchRes = await api.searchItems({
          q: item.title,
          page: 1,
          pageSize: DEFAULTS.PAGE_SIZE,
        });
        resolved = searchRes.items.find((candidate) => candidate.canonicalUrl === item.canonicalUrl);
        if (resolved) {
          const foundItem = resolved;
          setItemsByUrl((prev) => ({ ...prev, [foundItem.canonicalUrl]: foundItem }));
        }
      }

      if (!resolved) {
        setError({
          code: ErrorCode.NOT_FOUND,
          message: t('daily.bookmarkMissingError', { title: item.title }),
        });
        return;
      }

      if (resolved.bookmarked) {
        await api.deleteBookmark(resolved.id);
        const updated: ItemView = { ...resolved, bookmarked: false };
        setItemsByUrl((prev) => ({ ...prev, [updated.canonicalUrl]: updated }));
      } else {
        await api.upsertBookmark({
          itemId: resolved.id,
          note: resolved.bookmarkNote ?? undefined,
        });
        const updated: ItemView = { ...resolved, bookmarked: true };
        setItemsByUrl((prev) => ({ ...prev, [updated.canonicalUrl]: updated }));
      }
    } catch (err) {
      setError(toUiError(err, t));
    } finally {
      setBookmarkBusyUrl(null);
    }
  };

  const highlightedItems = useMemo(() => {
    if (!digest) {
      return [];
    }
    const collected: ProcessedItem[] = [];
    for (const section of digest.sections) {
      for (const item of section.items) {
        if (
          item.matchResult.priority === AlertPriority.URGENT ||
          item.matchResult.priority === AlertPriority.HIGH
        ) {
          collected.push(item);
        }
      }
    }
    return collected.sort((a, b) => {
      const rank = (p: AlertPriority) =>
        p === AlertPriority.URGENT ? 2 : p === AlertPriority.HIGH ? 1 : 0;
      const diff = rank(b.matchResult.priority) - rank(a.matchResult.priority);
      if (diff !== 0) {
        return diff;
      }
      return b.matchResult.importanceScore - a.matchResult.importanceScore;
    });
  }, [digest]);

  const activeStageIndex = latestRunProgress
    ? RUN_STAGE_ORDER.indexOf(latestRunProgress.stage)
    : -1;
  const activeStageKey = latestRunProgress ? stageTranslationKey(latestRunProgress.stage) : null;

  return (
    <div className="space-y-6">
      {/* Header & Manual Run Trigger Bar */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-4 sm:p-5">
        <div className="flex flex-col gap-4 lg:flex-row lg:items-center lg:justify-between">
          <div className="space-y-1">
            <h1 className="text-xl font-bold tracking-tight text-white">{t('daily.title')}</h1>
            <p className="text-xs text-slate-400">{t('daily.subtitle')}</p>
          </div>

          <div className="flex flex-wrap items-center gap-3">
            <label className="flex items-center gap-2 rounded-lg border border-slate-700/80 bg-slate-950/70 px-3 py-1.5 text-xs text-slate-200">
              <Calendar className="h-4 w-4 text-indigo-400" />
              <span className="sr-only">{t('daily.digestDateAria')}</span>
              <input
                type="date"
                aria-label={t('daily.digestDateAria')}
                value={selectedDate}
                onChange={(event) => setSelectedDate(event.target.value)}
                className="bg-transparent text-xs text-slate-100 focus:outline-none"
              />
            </label>

            {availableDigests.length > 0 && (
              <select
                aria-label={t('daily.availableDatesAria')}
                value={
                  availableDigests.some((d) => d.digestDate === selectedDate) ? selectedDate : ''
                }
                onChange={(event) => {
                  if (event.target.value) {
                    setSelectedDate(event.target.value);
                  }
                }}
                className="rounded-lg border border-slate-700/80 bg-slate-950/70 px-3 py-1.5 text-xs text-slate-200 focus:border-indigo-500 focus:outline-none"
              >
                <option value="" disabled>
                  {t('daily.selectSavedDate')}
                </option>
                {availableDigests.map((summary) => (
                  <option key={summary.id} value={summary.digestDate}>
                    {summary.digestDate} ({t('common.itemsCount', { count: summary.itemCount })})
                  </option>
                ))}
              </select>
            )}

            <label className="flex cursor-pointer items-center gap-1.5 text-xs text-slate-300">
              <input
                type="checkbox"
                checked={forceRefresh}
                onChange={(event) => setForceRefresh(event.target.checked)}
                className="rounded border-slate-600 bg-slate-900 text-indigo-500 focus:ring-indigo-500"
              />
              {t('daily.forceRefresh')}
            </label>

            <label className="flex cursor-pointer items-center gap-1.5 text-xs text-slate-300">
              <input
                type="checkbox"
                checked={notifyOnRun}
                onChange={(event) => setNotifyOnRun(event.target.checked)}
                className="rounded border-slate-600 bg-slate-900 text-indigo-500 focus:ring-indigo-500"
              />
              {t('daily.notify')}
            </label>

            <button
              type="button"
              onClick={() => void handleTriggerRun()}
              disabled={triggeringRun}
              className="inline-flex items-center gap-2 rounded-lg bg-indigo-600 px-3.5 py-2 text-xs font-semibold text-white transition-colors hover:bg-indigo-500 disabled:cursor-not-allowed disabled:opacity-60"
            >
              {triggeringRun ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Play className="h-3.5 w-3.5" />
              )}
              {triggeringRun ? t('daily.running') : t('daily.runNow')}
            </button>
          </div>
        </div>

        {/* Quick Date Pills */}
        {availableDigests.length > 0 && (
          <div className="mt-4 flex flex-wrap items-center gap-2 border-t border-slate-800/80 pt-3">
            <span className="text-[11px] font-medium uppercase tracking-wider text-slate-500">
              {t('daily.recentDates')}
            </span>
            {availableDigests.slice(0, 8).map((summary) => {
              const active = summary.digestDate === selectedDate;
              return (
                <button
                  key={summary.id}
                  type="button"
                  onClick={() => setSelectedDate(summary.digestDate)}
                  className={`rounded-full px-2.5 py-1 text-xs font-medium transition-colors ${
                    active
                      ? 'bg-indigo-600 text-white'
                      : 'bg-slate-800/80 text-slate-300 hover:bg-slate-700'
                  }`}
                >
                  {summary.digestDate}
                </button>
              );
            })}
          </div>
        )}

        {/* Live SSE Run Progress Bar */}
        {latestRunProgress && (
          <div
            className="mt-4 rounded-lg border border-indigo-500/30 bg-indigo-950/30 p-3"
            aria-live="polite"
          >
            <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
              <div className="flex items-center gap-2 font-medium text-indigo-200">
                <RefreshCw
                  className={`h-3.5 w-3.5 ${
                    latestRunProgress.status === RunStatus.RUNNING ? 'animate-spin' : ''
                  }`}
                />
                <span>
                  {t('daily.runLabel')}{' '}
                  <span className="font-mono">{latestRunProgress.runId.slice(0, 8)}</span> •{' '}
                  {t('daily.stageLabel')}{' '}
                  <strong className="text-white">
                    {activeStageKey ? t(activeStageKey) : latestRunProgress.stage}
                  </strong>
                </span>
              </div>
              <div className="flex items-center gap-3 text-slate-300">
                <span>{t('daily.fetchedCount', { count: latestRunProgress.itemsFetched })}</span>
                <span>{t('daily.newCount', { count: latestRunProgress.itemsNew })}</span>
                <span className="rounded bg-slate-800 px-2 py-0.5 font-mono text-[11px]">
                  {labelRunStatus(latestRunProgress.status)}
                </span>
              </div>
            </div>
            <div className="mt-2.5 grid grid-cols-7 gap-1">
              {RUN_STAGE_ORDER.map((stage, idx) => {
                const done = activeStageIndex >= idx;
                return (
                  <div key={stage} className="space-y-1">
                    <div
                      className={`h-1.5 rounded-full transition-colors ${
                        done ? 'bg-indigo-500' : 'bg-slate-800'
                      }`}
                    />
                    <div className="truncate text-center text-[10px] text-slate-400">
                      {t(`sse.stage.${stage}` as TranslationKey)}
                    </div>
                  </div>
                );
              })}
            </div>
            {latestRunProgress.message && (
              <p className="mt-2 text-xs text-indigo-200/90">{latestRunProgress.message}</p>
            )}
          </div>
        )}
      </section>

      {/* Structured Error Banner */}
      {error && (
        <div
          role="alert"
          className="flex items-start justify-between gap-3 rounded-xl border border-rose-500/40 bg-rose-950/40 p-4 text-sm text-rose-200"
        >
          <div className="flex items-start gap-2.5">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-rose-400" />
            <div className="space-y-1">
              <div className="font-semibold">
                {error.code ? `${error.code}: ` : ''}
                {error.message}
              </div>
              {error.requestId && (
                <div className="font-mono text-xs text-rose-300/80">
                  {t('common.requestId', { id: error.requestId })}
                </div>
              )}
            </div>
          </div>
          <button
            type="button"
            onClick={() => setError(null)}
            className="text-xs text-rose-300 hover:text-white"
          >
            {t('common.dismiss')}
          </button>
        </div>
      )}

      {/* Loading State */}
      {(loadingDates || loadingDigest) && !digest && (
        <div className="flex items-center justify-center gap-2 rounded-xl border border-slate-800 bg-slate-900/50 py-14 text-sm text-slate-400">
          <Loader2 className="h-4 w-4 animate-spin text-indigo-400" />
          {t('daily.loading')}
        </div>
      )}

      {/* Empty / 404 State */}
      {!loadingDigest && notFoundForDate && (
        <div className="rounded-xl border border-dashed border-slate-700 bg-slate-900/40 p-8 text-center">
          <Sparkles className="mx-auto h-8 w-8 text-indigo-400" />
          <h2 className="mt-3 text-base font-semibold text-white">
            {t('daily.emptyTitle', { date: selectedDate })}
          </h2>
          <p className="mx-auto mt-1 max-w-md text-xs text-slate-400">{t('daily.emptyDesc')}</p>
          <div className="mt-5 flex flex-wrap items-center justify-center gap-3">
            <button
              type="button"
              onClick={() => void handleTriggerRun()}
              disabled={triggeringRun}
              className="inline-flex items-center gap-2 rounded-lg bg-indigo-600 px-4 py-2 text-xs font-semibold text-white hover:bg-indigo-500 disabled:opacity-60"
            >
              <Play className="h-3.5 w-3.5" />
              {t('daily.triggerFirstRun')}
            </button>
            {onNavigate && (
              <button
                type="button"
                onClick={() => onNavigate(APP_ROUTES.SETTINGS)}
                className="rounded-lg border border-slate-700 bg-slate-800/80 px-4 py-2 text-xs font-medium text-slate-200 hover:bg-slate-700"
              >
                {t('layout.openSettings')}
              </button>
            )}
          </div>
        </div>
      )}

      {/* Digest Content */}
      {digest && (
        <>
          {/* Urgent / High Watchlist Highlight Banner */}
          {highlightedItems.length > 0 && (
            <section
              aria-label={t('daily.highlightsAria')}
              className="rounded-xl border border-rose-500/40 bg-gradient-to-r from-rose-950/60 via-amber-950/40 to-slate-900/80 p-4 sm:p-5"
            >
              <div className="flex items-center gap-2 text-sm font-semibold text-rose-200">
                <Flame className="h-4 w-4 text-rose-400" />
                <span>{t('daily.highlightsTitle', { count: highlightedItems.length })}</span>
              </div>
              <div className="mt-3 grid gap-2.5 sm:grid-cols-2">
                {highlightedItems.map((item) => {
                  const safeUrl = sanitizeHttpUrl(item.canonicalUrl);
                  return (
                    <div
                      key={`${item.contentHash}-${item.canonicalUrl}`}
                      className="flex flex-col justify-between gap-2 rounded-lg border border-rose-500/25 bg-slate-950/70 p-3"
                    >
                      <div className="space-y-1.5">
                        <div className="flex items-center justify-between gap-2">
                          <span
                            className={`rounded-full border px-2 py-0.5 text-[10px] font-semibold uppercase ${priorityBadgeClass(
                              item.matchResult.priority,
                            )}`}
                          >
                            {labelAlertPriority(item.matchResult.priority)}
                          </span>
                          <span className="text-[11px] text-slate-400">
                            {item.sourceName ?? t('daily.sourceFallback')} • {t('common.score')}{' '}
                            {item.matchResult.importanceScore}
                          </span>
                        </div>
                        <div className="text-sm font-medium text-white">{item.title}</div>
                        {item.matchResult.matchedKeywords.length > 0 && (
                          <div className="flex flex-wrap gap-1">
                            {item.matchResult.matchedKeywords.map((kw) => (
                              <span
                                key={kw}
                                className="rounded bg-rose-500/20 px-1.5 py-0.5 text-[10px] font-medium text-rose-200"
                              >
                                #{kw}
                              </span>
                            ))}
                          </div>
                        )}
                      </div>
                      {safeUrl && (
                        <button
                          type="button"
                          onClick={() => handleOpenExternal(safeUrl)}
                          className="inline-flex w-fit items-center gap-1 text-xs font-medium text-indigo-300 hover:text-indigo-200"
                        >
                          {t('daily.readOriginal')}
                          <ExternalLink className="h-3 w-3" />
                        </button>
                      )}
                    </div>
                  );
                })}
              </div>
            </section>
          )}

          {/* Degraded Deterministic Fallback Warning */}
          {digest.degradedFallback && (
            <div
              role="status"
              className="flex items-start gap-2.5 rounded-xl border border-amber-500/40 bg-amber-950/40 p-4 text-xs text-amber-200"
            >
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-400" />
              <div>
                <span className="font-semibold text-amber-100">
                  {t('daily.fallbackActiveTitle')}
                </span>
                <p className="mt-0.5 text-amber-200/90">
                  {t('daily.fallbackActiveDesc')}{' '}
                  <span className="font-mono">{digest.modelUsed}</span>.
                </p>
              </div>
            </div>
          )}

          {/* AI Executive Overview Card */}
          <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5">
            <div className="flex flex-col gap-3 border-b border-slate-800 pb-4 sm:flex-row sm:items-start sm:justify-between">
              <div className="space-y-1.5">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="rounded-full bg-indigo-500/15 px-2.5 py-0.5 text-xs font-medium text-indigo-300">
                    {digest.digestDate}
                  </span>
                  <span className="rounded-full bg-slate-800 px-2.5 py-0.5 font-mono text-[11px] text-slate-300">
                    {digest.modelUsed}
                  </span>
                </div>
                <h2 className="text-lg font-bold text-white">{digest.title}</h2>
                <div className="flex flex-wrap gap-4 text-xs text-slate-400">
                  <span>{t('daily.itemsStat', { count: digest.itemCount })}</span>
                  <span>{t('daily.highlightedStat', { count: digest.highlightedCount })}</span>
                  <span>
                    {t('daily.tokensStat', {
                      prompt: digest.promptTokens,
                      completion: digest.completionTokens,
                    })}
                  </span>
                </div>
              </div>

              <button
                type="button"
                onClick={() => void handleCopyMarkdown()}
                disabled={copyStatus === 'copying'}
                className="inline-flex shrink-0 items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-800/80 px-3 py-1.5 text-xs font-medium text-slate-200 transition-colors hover:bg-slate-700 disabled:opacity-60"
              >
                {copyStatus === 'copied' ? (
                  <>
                    <Check className="h-3.5 w-3.5 text-emerald-400" />
                    <span className="text-emerald-300">{t('common.copied')}</span>
                  </>
                ) : (
                  <>
                    <Copy className="h-3.5 w-3.5" />
                    <span>
                      {copyStatus === 'copying'
                        ? t('daily.copyingMarkdown')
                        : t('daily.copyMarkdown')}
                    </span>
                  </>
                )}
              </button>
            </div>

            <div className="mt-4">
              <SafeMarkdown content={digest.overviewMarkdown} bridge={bridge} />
            </div>
          </section>

          {/* Categorized Source Sections */}
          <section className="space-y-4">
            <h3 className="text-sm font-semibold uppercase tracking-wider text-slate-400">
              {t('daily.sourceSectionsTitle', { count: digest.sections.length })}
            </h3>
            {digest.sections.map((section, idx) => (
              <article
                key={`${section.sourceId ?? section.sourceName}-${idx}`}
                className="rounded-xl border border-slate-800 bg-slate-900/60 p-5"
              >
                <div className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-800/80 pb-3">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="rounded bg-indigo-500/15 px-2 py-0.5 text-[11px] font-semibold text-indigo-300">
                      {section.category}
                    </span>
                    {section.sourceId && onNavigate ? (
                      <button
                        type="button"
                        onClick={() =>
                          onNavigate(APP_ROUTES.SOURCES, { sourceId: section.sourceId ?? '' })
                        }
                        className="text-base font-semibold text-white hover:text-indigo-300"
                      >
                        {section.sourceName}
                      </button>
                    ) : (
                      <h4 className="text-base font-semibold text-white">{section.sourceName}</h4>
                    )}
                  </div>
                  <span className="text-xs text-slate-400">
                    {t('common.itemsCount', { count: section.items.length })}
                  </span>
                </div>

                {section.summaryMarkdown && (
                  <div className="mt-3">
                    <SafeMarkdown content={section.summaryMarkdown} bridge={bridge} />
                  </div>
                )}

                {section.keyPoints.length > 0 && (
                  <div className="mt-3 rounded-lg border border-slate-800/80 bg-slate-950/50 p-3">
                    <div className="text-[11px] font-semibold uppercase tracking-wider text-slate-400">
                      {t('daily.keyPoints')}
                    </div>
                    <ul className="mt-1.5 list-disc space-y-1 pl-4 text-xs text-slate-300">
                      {section.keyPoints.map((point, pointIdx) => (
                        <li key={pointIdx}>{point}</li>
                      ))}
                    </ul>
                  </div>
                )}

                {section.items.length > 0 && (
                  <div className="mt-4 space-y-2.5">
                    {section.items.map((item) => {
                      const safeUrl = sanitizeHttpUrl(item.canonicalUrl);
                      const indexedItem = itemsByUrl[item.canonicalUrl];
                      const isBookmarked = indexedItem?.bookmarked ?? false;
                      const isBookmarkBusy = bookmarkBusyUrl === item.canonicalUrl;

                      return (
                        <div
                          key={`${item.contentHash}-${item.canonicalUrl}`}
                          className="rounded-lg border border-slate-800/80 bg-slate-950/60 p-3.5 transition-colors hover:border-slate-700"
                        >
                          <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
                            <div className="space-y-1">
                              <div className="flex flex-wrap items-center gap-2">
                                <span
                                  className={`rounded-full border px-2 py-0.5 text-[10px] font-semibold uppercase ${priorityBadgeClass(
                                    item.matchResult.priority,
                                  )}`}
                                >
                                  {labelAlertPriority(item.matchResult.priority)}
                                </span>
                                <span className="text-[11px] text-slate-400">
                                  {t('common.score')}: {item.matchResult.importanceScore}
                                </span>
                                {item.author && (
                                  <span className="text-[11px] text-slate-400">
                                    {t('common.byAuthor', { author: item.author })}
                                  </span>
                                )}
                                {item.publishedAt && (
                                  <span className="text-[11px] text-slate-500">
                                    {formatDateTime(item.publishedAt) || t('common.unknownTime')}
                                  </span>
                                )}
                              </div>

                              {safeUrl ? (
                                <a
                                  href={safeUrl}
                                  target="_blank"
                                  rel="noopener noreferrer"
                                  onClick={(event) => {
                                    event.preventDefault();
                                    handleOpenExternal(safeUrl);
                                  }}
                                  className="inline-flex items-center gap-1.5 text-sm font-medium text-slate-100 hover:text-indigo-300"
                                >
                                  <span>{item.title}</span>
                                  <ExternalLink className="h-3.5 w-3.5 shrink-0 text-indigo-400" />
                                </a>
                              ) : (
                                <div className="text-sm font-medium text-slate-200">
                                  {item.title}
                                </div>
                              )}

                              {item.summary && (
                                <p className="text-xs leading-relaxed text-slate-400">
                                  {item.summary}
                                </p>
                              )}

                              {item.matchResult.matchedKeywords.length > 0 && (
                                <div className="flex flex-wrap gap-1 pt-1">
                                  {item.matchResult.matchedKeywords.map((kw) => (
                                    <span
                                      key={kw}
                                      className="rounded bg-indigo-500/15 px-1.5 py-0.5 text-[10px] font-medium text-indigo-300"
                                    >
                                      #{kw}
                                    </span>
                                  ))}
                                </div>
                              )}
                            </div>

                            <div className="flex shrink-0 items-center gap-2">
                              <button
                                type="button"
                                onClick={() => void handleToggleBookmark(item)}
                                disabled={isBookmarkBusy}
                                aria-label={
                                  isBookmarked
                                    ? t('common.bookmarkAriaRemove')
                                    : t('common.bookmarkAriaAdd')
                                }
                                className={`inline-flex items-center gap-1 rounded-lg border px-2.5 py-1 text-xs font-medium transition-colors ${
                                  isBookmarked
                                    ? 'border-amber-500/50 bg-amber-500/15 text-amber-300 hover:bg-amber-500/25'
                                    : 'border-slate-700 bg-slate-900 text-slate-300 hover:bg-slate-800'
                                } disabled:opacity-50`}
                              >
                                {isBookmarkBusy ? (
                                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                                ) : isBookmarked ? (
                                  <BookmarkCheck className="h-3.5 w-3.5" />
                                ) : (
                                  <Bookmark className="h-3.5 w-3.5" />
                                )}
                                {isBookmarked ? t('common.bookmarked') : t('common.bookmark')}
                              </button>
                            </div>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                )}
              </article>
            ))}
          </section>
        </>
      )}
    </div>
  );
}

export default DailyDigestPage;
