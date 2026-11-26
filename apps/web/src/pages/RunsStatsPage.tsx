import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Clock,
  RefreshCw,
  RotateCcw,
  Send,
  Sparkles,
  XCircle,
} from 'lucide-react';
import { ApiError, type ApiClient } from '../api/client';
import { DEFAULTS } from '../constants/routes';
import { type TranslateFn, useI18n } from '../i18n';
import type {
  DeliveryLogRecord,
  DigestRunView,
  PagedResult,
  RunProgressEvent,
  RunStatsView,
  SourceHealthStat,
} from '../types/api';
import { DeliveryStatus, FetchOutcome, RunStatus, SourceScope } from '../types/enums';

export interface RunsStatsPageProps {
  api: ApiClient;
  latestRunProgress?: RunProgressEvent | null;
}

const TERMINAL_RUN_STATUSES: ReadonlySet<RunStatus> = new Set([
  RunStatus.SUCCEEDED,
  RunStatus.PARTIAL,
  RunStatus.FAILED,
]);

function formatErrorMessage(err: unknown, t: TranslateFn): string {
  if (err instanceof ApiError) {
    return err.requestId
      ? `${err.message} (${t('common.requestId', { id: err.requestId })})`
      : err.message;
  }
  if (err instanceof Error) {
    return err.message;
  }
  return t('runs.loadErrorFallback');
}

function getRunStatusBadgeClass(status: RunStatus): string {
  switch (status) {
    case RunStatus.SUCCEEDED:
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30';
    case RunStatus.PARTIAL:
      return 'bg-amber-500/15 text-amber-300 border-amber-500/30';
    case RunStatus.FAILED:
      return 'bg-rose-500/15 text-rose-300 border-rose-500/30';
    case RunStatus.RUNNING:
      return 'bg-indigo-500/15 text-indigo-300 border-indigo-500/30';
  }
}

function getFetchOutcomeBadgeClass(outcome: FetchOutcome | null): string {
  switch (outcome) {
    case FetchOutcome.UPDATED:
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30';
    case FetchOutcome.NOT_MODIFIED_304:
    case FetchOutcome.SKIPPED_FRESH:
      return 'bg-sky-500/15 text-sky-300 border-sky-500/30';
    case FetchOutcome.FAILED:
      return 'bg-rose-500/15 text-rose-300 border-rose-500/30';
    case null:
      return 'bg-slate-700/40 text-slate-300 border-slate-600/40';
  }
}

function getDeliveryStatusBadgeClass(status: DeliveryStatus): string {
  switch (status) {
    case DeliveryStatus.SENT:
      return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30';
    case DeliveryStatus.PENDING:
      return 'bg-amber-500/15 text-amber-300 border-amber-500/30';
    case DeliveryStatus.FAILED:
      return 'bg-rose-500/15 text-rose-300 border-rose-500/30';
  }
}

export function RunsStatsPage({ api, latestRunProgress }: RunsStatsPageProps) {
  const {
    t,
    formatDateTime,
    formatShortTime,
    labelSourceType,
    labelSourceScope,
    labelChannelType,
    labelRunType,
    labelRunStatus,
    labelDeliveryStatus,
    labelFetchOutcome,
  } = useI18n();
  const tRef = useRef(t);
  tRef.current = t;

  const [stats, setStats] = useState<RunStatsView | null>(null);
  const [runsPage, setRunsPage] = useState<PagedResult<DigestRunView> | null>(null);
  const [page, setPage] = useState<number>(1);
  const [loading, setLoading] = useState<boolean>(true);
  const [refreshing, setRefreshing] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);
  const [actionMessage, setActionMessage] = useState<string | null>(null);
  const [resendingIds, setResendingIds] = useState<Record<string, boolean>>({});
  const [collapsedRunIds, setCollapsedRunIds] = useState<Record<string, boolean>>({});

  const lastTerminalKeyRef = useRef<string | null>(null);

  const loadData = useCallback(
    async (targetPage: number, background = false) => {
      if (background) {
        setRefreshing(true);
      } else {
        setLoading(true);
      }
      setError(null);
      try {
        const [statsRes, runsRes] = await Promise.all([
          api.getRunStats(),
          api.listRuns({ page: targetPage, pageSize: DEFAULTS.PAGE_SIZE }),
        ]);
        setStats(statsRes);
        setRunsPage(runsRes);
      } catch (err) {
        setError(formatErrorMessage(err, tRef.current));
      } finally {
        setLoading(false);
        setRefreshing(false);
      }
    },
    [api],
  );

  useEffect(() => {
    void loadData(page);
  }, [loadData, page]);

  useEffect(() => {
    if (!latestRunProgress) {
      return;
    }
    if (!TERMINAL_RUN_STATUSES.has(latestRunProgress.status)) {
      return;
    }
    const key = `${latestRunProgress.runId}:${latestRunProgress.status}`;
    if (lastTerminalKeyRef.current === key) {
      return;
    }
    lastTerminalKeyRef.current = key;
    void loadData(page, true);
  }, [latestRunProgress, loadData, page]);

  const handleResend = useCallback(
    async (delivery: DeliveryLogRecord) => {
      if (delivery.digestId === null || delivery.channelId === null) {
        return;
      }
      setResendingIds((prev) => ({ ...prev, [delivery.id]: true }));
      setError(null);
      setActionMessage(null);
      try {
        const updated = await api.resendDelivery(delivery.id);
        setRunsPage((prev) => {
          if (!prev) {
            return prev;
          }
          return {
            ...prev,
            items: prev.items.map((run) =>
              run.id !== delivery.runId
                ? run
                : {
                    ...run,
                    deliveries: run.deliveries.map((d) => (d.id === updated.id ? updated : d)),
                  },
            ),
          };
        });
        setActionMessage(
          t('runs.deliveryUpdated', {
            id: updated.id,
            status: labelDeliveryStatus(updated.status),
          }),
        );
        void loadData(page, true);
      } catch (err) {
        setError(formatErrorMessage(err, t));
      } finally {
        setResendingIds((prev) => ({ ...prev, [delivery.id]: false }));
      }
    },
    [api, labelDeliveryStatus, loadData, page, t],
  );

  const toggleRunDeliveries = (runId: string) => {
    setCollapsedRunIds((prev) => ({ ...prev, [runId]: !prev[runId] }));
  };

  const totalTokens = (stats?.totalPromptTokens ?? 0) + (stats?.totalCompletionTokens ?? 0);
  const promptSharePct =
    totalTokens > 0 ? Math.round(((stats?.totalPromptTokens ?? 0) / totalTokens) * 100) : 0;
  const completionSharePct = totalTokens > 0 ? 100 - promptSharePct : 0;

  const totalDeliveries = (stats?.deliveriesSent ?? 0) + (stats?.deliveriesFailed ?? 0);
  const deliverySuccessRate =
    totalDeliveries > 0
      ? `${Math.round(((stats?.deliveriesSent ?? 0) / totalDeliveries) * 100)}%`
      : t('runs.na');

  const runs = runsPage?.items ?? [];

  const chartRuns = useMemo(() => [...runs].slice(0, 12).reverse(), [runs]);
  const maxRunTokens = useMemo(() => {
    if (chartRuns.length === 0) {
      return 1;
    }
    return Math.max(
      ...chartRuns.map((run) => Math.max(0, run.promptTokens) + Math.max(0, run.completionTokens)),
      1,
    );
  }, [chartRuns]);

  const svgWidth = Math.max(chartRuns.length * 56, 320);
  const svgHeight = 160;
  const plotHeight = 120;

  return (
    <div className="space-y-6">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-2xl font-bold tracking-tight text-white">{t('runs.title')}</h1>
          <p className="mt-1 text-sm text-slate-400">{t('runs.subtitle')}</p>
        </div>
        <button
          type="button"
          onClick={() => void loadData(page, true)}
          disabled={loading || refreshing}
          className="inline-flex items-center gap-2 rounded-lg border border-slate-700 bg-slate-800 px-3.5 py-2 text-sm font-medium text-slate-100 transition hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-50"
        >
          <RefreshCw className={`h-4 w-4 ${refreshing ? 'animate-spin' : ''}`} />
          {t('common.refresh')}
        </button>
      </header>

      {error && (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-lg border border-rose-500/40 bg-rose-950/50 px-4 py-3 text-sm text-rose-200"
        >
          <XCircle className="mt-0.5 h-4 w-4 shrink-0 text-rose-400" />
          <span>{error}</span>
        </div>
      )}

      {actionMessage && (
        <div
          role="status"
          className="flex items-start gap-2 rounded-lg border border-emerald-500/40 bg-emerald-950/40 px-4 py-3 text-sm text-emerald-200"
        >
          <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-emerald-400" />
          <span>{actionMessage}</span>
        </div>
      )}

      {loading && !stats ? (
        <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-8 text-center text-sm text-slate-400">
          {t('runs.loadingStats')}
        </div>
      ) : (
        <>
          {/* KPI Summary Cards */}
          <section aria-label={t('runs.kpiAria')} className="grid gap-4 sm:grid-cols-3">
            <div className="rounded-xl border border-slate-800 bg-slate-900/70 p-5">
              <div className="flex items-center justify-between text-xs font-medium uppercase tracking-wider text-slate-400">
                <span>{t('runs.totalRuns')}</span>
                <Activity className="h-4 w-4 text-indigo-400" />
              </div>
              <div className="mt-2 text-3xl font-bold text-white">
                {(stats?.totalRuns ?? 0).toLocaleString()}
              </div>
              <div className="mt-2 flex items-center gap-3 text-xs text-slate-400">
                <span className="text-emerald-300">
                  {t('runs.succeededCount', { count: stats?.succeededRuns ?? 0 })}
                </span>
                <span className="text-rose-300">
                  {t('runs.failedCount', { count: stats?.failedRuns ?? 0 })}
                </span>
              </div>
            </div>

            <div className="rounded-xl border border-slate-800 bg-slate-900/70 p-5">
              <div className="flex items-center justify-between text-xs font-medium uppercase tracking-wider text-slate-400">
                <span>{t('runs.totalTokens')}</span>
                <Sparkles className="h-4 w-4 text-cyan-400" />
              </div>
              <div className="mt-2 text-3xl font-bold text-white">
                {totalTokens.toLocaleString()}
              </div>
              <div className="mt-2 flex flex-wrap items-center gap-3 text-xs text-slate-400">
                <span className="text-indigo-300">
                  {t('runs.promptTokensInline', {
                    count: (stats?.totalPromptTokens ?? 0).toLocaleString(),
                  })}
                </span>
                <span className="text-cyan-300">
                  {t('runs.completionTokensInline', {
                    count: (stats?.totalCompletionTokens ?? 0).toLocaleString(),
                  })}
                </span>
              </div>
            </div>

            <div className="rounded-xl border border-slate-800 bg-slate-900/70 p-5">
              <div className="flex items-center justify-between text-xs font-medium uppercase tracking-wider text-slate-400">
                <span>{t('runs.deliverySuccessRate')}</span>
                <Send className="h-4 w-4 text-emerald-400" />
              </div>
              <div className="mt-2 text-3xl font-bold text-white">{deliverySuccessRate}</div>
              <div className="mt-2 flex items-center gap-3 text-xs text-slate-400">
                <span className="text-emerald-300">
                  {t('runs.sentCount', { count: stats?.deliveriesSent ?? 0 })}
                </span>
                <span className="text-rose-300">
                  {t('runs.failedCount', { count: stats?.deliveriesFailed ?? 0 })}
                </span>
              </div>
            </div>
          </section>

          {/* Token Consumption Visual Charts */}
          <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <h2 className="text-base font-semibold text-white">
                  {t('runs.tokenConsumption')}
                </h2>
                <p className="text-xs text-slate-400">{t('runs.tokenSubtitle')}</p>
              </div>
              <div className="flex items-center gap-4 text-xs text-slate-300">
                <span className="inline-flex items-center gap-1.5">
                  <span className="h-2.5 w-2.5 rounded-sm bg-indigo-500" />
                  {t('runs.promptTokens')}
                </span>
                <span className="inline-flex items-center gap-1.5">
                  <span className="h-2.5 w-2.5 rounded-sm bg-cyan-400" />
                  {t('runs.completionTokens')}
                </span>
              </div>
            </div>

            {/* Overall Share Bar */}
            <div>
              <div className="mb-1.5 flex justify-between text-xs text-slate-300">
                <span>
                  {t('runs.promptShare', {
                    count: (stats?.totalPromptTokens ?? 0).toLocaleString(),
                    pct: promptSharePct,
                  })}
                </span>
                <span>
                  {t('runs.completionShare', {
                    count: (stats?.totalCompletionTokens ?? 0).toLocaleString(),
                    pct: completionSharePct,
                  })}
                </span>
              </div>
              <div
                className="flex h-3 w-full overflow-hidden rounded-full bg-slate-800"
                role="img"
                aria-label={t('runs.tokenShareAria', {
                  promptPct: promptSharePct,
                  completionPct: completionSharePct,
                })}
              >
                {totalTokens > 0 && (
                  <>
                    <div
                      className="bg-indigo-500 transition-all"
                      style={{ width: `${promptSharePct}%` }}
                    />
                    <div
                      className="bg-cyan-400 transition-all"
                      style={{ width: `${completionSharePct}%` }}
                    />
                  </>
                )}
              </div>
            </div>

            {/* Stacked Per-Run SVG Chart */}
            {chartRuns.length === 0 ? (
              <p className="py-4 text-center text-xs text-slate-500">{t('runs.noChartRuns')}</p>
            ) : (
              <div className="overflow-x-auto">
                <svg
                  role="img"
                  aria-label={t('runs.chartAria')}
                  viewBox={`0 0 ${svgWidth} ${svgHeight}`}
                  className="h-44 w-full min-w-[320px]"
                >
                  {chartRuns.map((run, idx) => {
                    const slotWidth = svgWidth / chartRuns.length;
                    const barWidth = Math.min(34, Math.max(14, slotWidth * 0.55));
                    const x = idx * slotWidth + (slotWidth - barWidth) / 2;
                    const promptVal = Math.max(0, run.promptTokens);
                    const completionVal = Math.max(0, run.completionTokens);
                    const totalVal = promptVal + completionVal;
                    const promptHeight =
                      totalVal > 0 ? Math.round((promptVal / maxRunTokens) * plotHeight) : 0;
                    const completionHeight =
                      totalVal > 0 ? Math.round((completionVal / maxRunTokens) * plotHeight) : 0;
                    const promptY = 12 + plotHeight - promptHeight;
                    const completionY = promptY - completionHeight;

                    return (
                      <g key={run.id}>
                        <title>
                          {`${labelRunType(run.runType)} (${formatDateTime(run.startedAt)}) — ${t('runs.promptTokens')}: ${promptVal.toLocaleString()}, ${t('runs.completionTokens')}: ${completionVal.toLocaleString()}`}
                        </title>
                        <rect
                          x={x}
                          y={12}
                          width={barWidth}
                          height={plotHeight}
                          rx={4}
                          fill="rgba(30, 41, 59, 0.7)"
                        />
                        {promptHeight > 0 && (
                          <rect
                            x={x}
                            y={promptY}
                            width={barWidth}
                            height={promptHeight}
                            rx={completionHeight > 0 ? 0 : 4}
                            fill="#6366f1"
                          />
                        )}
                        {completionHeight > 0 && (
                          <rect
                            x={x}
                            y={completionY}
                            width={barWidth}
                            height={completionHeight}
                            rx={4}
                            fill="#22d3ee"
                          />
                        )}
                        <text
                          x={x + barWidth / 2}
                          y={148}
                          textAnchor="middle"
                          fontSize="10"
                          fill="#94a3b8"
                        >
                          {formatShortTime(run.startedAt)}
                        </text>
                      </g>
                    );
                  })}
                </svg>
              </div>
            )}
          </section>

          {/* Source Health Table */}
          <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5">
            <h2 className="text-base font-semibold text-white">{t('runs.sourceHealth')}</h2>
            <p className="mt-0.5 text-xs text-slate-400">{t('runs.sourceHealthSubtitle')}</p>
            <div className="mt-4 overflow-x-auto">
              <table className="min-w-full divide-y divide-slate-800 text-left text-xs">
                <thead>
                  <tr className="text-slate-400">
                    <th className="py-2 pr-4 font-medium">{t('runs.colSource')}</th>
                    <th className="py-2 pr-4 font-medium">{t('runs.colType')}</th>
                    <th className="py-2 pr-4 font-medium">{t('runs.colScope')}</th>
                    <th className="py-2 pr-4 font-medium">{t('runs.colEnabled')}</th>
                    <th className="py-2 pr-4 font-medium">{t('runs.colLastStatus')}</th>
                    <th className="py-2 pr-4 font-medium">{t('runs.colFailures')}</th>
                    <th className="py-2 pr-4 font-medium">{t('runs.colLastFetched')}</th>
                    <th className="py-2 font-medium">{t('runs.colLastError')}</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-slate-800/70 text-slate-200">
                  {!stats || stats.sourceHealth.length === 0 ? (
                    <tr>
                      <td colSpan={8} className="py-6 text-center text-slate-500">
                        {t('runs.noSources')}
                      </td>
                    </tr>
                  ) : (
                    stats.sourceHealth.map((src: SourceHealthStat) => (
                      <tr key={src.sourceId}>
                        <td className="py-2.5 pr-4 font-medium text-white">{src.sourceName}</td>
                        <td className="py-2.5 pr-4 font-mono text-slate-300">
                          {labelSourceType(src.sourceType)}
                        </td>
                        <td className="py-2.5 pr-4">
                          <span className="rounded border border-slate-700 bg-slate-800 px-2 py-0.5 text-[11px] text-slate-300">
                            {labelSourceScope(
                              src.scope === SourceScope.SHARED_PUBLIC
                                ? SourceScope.SHARED_PUBLIC
                                : SourceScope.USER_PRIVATE,
                            )}
                          </span>
                        </td>
                        <td className="py-2.5 pr-4">
                          {src.enabled ? (
                            <span className="text-emerald-300">{t('common.enabled')}</span>
                          ) : (
                            <span className="text-slate-500">{t('common.paused')}</span>
                          )}
                        </td>
                        <td className="py-2.5 pr-4">
                          <span
                            className={`inline-block rounded-full border px-2 py-0.5 text-[11px] font-medium ${getFetchOutcomeBadgeClass(src.lastStatus)}`}
                          >
                            {labelFetchOutcome(src.lastStatus, 'pending')}
                          </span>
                        </td>
                        <td className="py-2.5 pr-4">
                          <span
                            className={
                              src.consecutiveFailures > 0
                                ? 'font-semibold text-rose-300'
                                : 'text-slate-300'
                            }
                          >
                            {src.consecutiveFailures}
                          </span>
                        </td>
                        <td className="py-2.5 pr-4 text-slate-400">
                          {src.lastFetchedAt ? formatDateTime(src.lastFetchedAt) : '—'}
                        </td>
                        <td className="max-w-xs truncate py-2.5 text-rose-300" title={src.lastError ?? ''}>
                          {src.lastError ?? '—'}
                        </td>
                      </tr>
                    ))
                  )}
                </tbody>
              </table>
            </div>
          </section>

          {/* Run History & Delivery Resend */}
          <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <h2 className="text-base font-semibold text-white">{t('runs.runHistory')}</h2>
                <p className="mt-0.5 text-xs text-slate-400">{t('runs.runHistorySubtitle')}</p>
              </div>
              {runsPage && (
                <div className="text-xs text-slate-400">
                  {t('runs.runsPagination', {
                    page: runsPage.page,
                    totalPages: Math.max(1, runsPage.totalPages),
                    total: runsPage.total,
                  })}
                </div>
              )}
            </div>

            <div className="mt-4 space-y-3">
              {runs.length === 0 ? (
                <div className="rounded-lg border border-slate-800 bg-slate-950/40 p-6 text-center text-xs text-slate-500">
                  {t('runs.noRuns')}
                </div>
              ) : (
                runs.map((run) => {
                  const isCollapsed = Boolean(collapsedRunIds[run.id]);
                  return (
                    <div
                      key={run.id}
                      className="rounded-lg border border-slate-800 bg-slate-950/50 p-4"
                    >
                      <div className="flex flex-wrap items-center justify-between gap-3">
                        <div className="flex flex-wrap items-center gap-2.5">
                          <span
                            className={`inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-medium ${getRunStatusBadgeClass(run.status)}`}
                          >
                            {labelRunStatus(run.status)}
                          </span>
                          <span className="text-sm font-semibold text-white">
                            {labelRunType(run.runType)}
                          </span>
                          <span className="inline-flex items-center gap-1 text-xs text-slate-400">
                            <Clock className="h-3.5 w-3.5" />
                            {t('runs.started', { time: formatDateTime(run.startedAt) || '—' })}
                          </span>
                          {run.finishedAt && (
                            <span className="text-xs text-slate-500">
                              {t('runs.finished', { time: formatDateTime(run.finishedAt) || '—' })}
                            </span>
                          )}
                        </div>

                        <div className="flex flex-wrap items-center gap-4 text-xs text-slate-300">
                          <span>
                            {t('runs.itemsSummary', {
                              fetched: run.itemsFetched,
                              newCount: run.itemsNew,
                            })}
                          </span>
                          <span>
                            {t('runs.tokensSummary', {
                              prompt: run.promptTokens.toLocaleString(),
                              completion: run.completionTokens.toLocaleString(),
                            })}
                          </span>
                        </div>
                      </div>

                      {run.errorSummary && (
                        <div className="mt-2 flex items-start gap-1.5 rounded border border-rose-500/30 bg-rose-950/30 px-3 py-2 text-xs text-rose-200">
                          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-rose-400" />
                          <span>{run.errorSummary}</span>
                        </div>
                      )}

                      {run.deliveries.length > 0 && (
                        <div className="mt-3 border-t border-slate-800/80 pt-3">
                          <button
                            type="button"
                            onClick={() => toggleRunDeliveries(run.id)}
                            className="inline-flex items-center gap-1 text-xs font-medium text-slate-300 hover:text-white"
                          >
                            {isCollapsed ? (
                              <ChevronRight className="h-3.5 w-3.5" />
                            ) : (
                              <ChevronDown className="h-3.5 w-3.5" />
                            )}
                            {t('runs.deliveriesCount', { count: run.deliveries.length })}
                          </button>

                          {!isCollapsed && (
                            <div className="mt-2 space-y-2">
                              {run.deliveries.map((delivery) => {
                                const canResend =
                                  delivery.digestId !== null && delivery.channelId !== null;
                                const isResending = Boolean(resendingIds[delivery.id]);
                                return (
                                  <div
                                    key={delivery.id}
                                    className="flex flex-wrap items-center justify-between gap-2 rounded border border-slate-800 bg-slate-900/60 px-3 py-2 text-xs"
                                  >
                                    <div className="flex flex-wrap items-center gap-2.5">
                                      <span className="font-mono font-medium text-slate-200">
                                        {labelChannelType(delivery.channelType)}
                                      </span>
                                      <span
                                        className={`rounded-full border px-2 py-0.5 text-[11px] font-medium ${getDeliveryStatusBadgeClass(delivery.status)}`}
                                      >
                                        {labelDeliveryStatus(delivery.status)}
                                      </span>
                                      <span className="text-slate-400">
                                        {formatDateTime(delivery.sentAt) || '—'}
                                      </span>
                                      {delivery.errorMessage && (
                                        <span className="text-rose-300">
                                          {delivery.errorMessage}
                                        </span>
                                      )}
                                    </div>

                                    <button
                                      type="button"
                                      disabled={!canResend || isResending}
                                      onClick={() => void handleResend(delivery)}
                                      title={
                                        canResend
                                          ? t('runs.resendTitle')
                                          : t('runs.resendUnavailableTitle')
                                      }
                                      className="inline-flex items-center gap-1.5 rounded border border-indigo-500/40 bg-indigo-500/10 px-2.5 py-1 text-xs font-medium text-indigo-200 transition hover:bg-indigo-500/20 disabled:cursor-not-allowed disabled:border-slate-700 disabled:bg-slate-800/60 disabled:text-slate-500"
                                    >
                                      <RotateCcw className="h-3 w-3" />
                                      {isResending ? t('runs.resending') : t('runs.resend')}
                                    </button>
                                  </div>
                                );
                              })}
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                  );
                })
              )}
            </div>

            {runsPage && runsPage.totalPages > 1 && (
              <div className="mt-4 flex items-center justify-end gap-2">
                <button
                  type="button"
                  disabled={page <= 1 || loading}
                  onClick={() => setPage((p) => Math.max(1, p - 1))}
                  className="rounded-lg border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {t('common.previous')}
                </button>
                <button
                  type="button"
                  disabled={page >= runsPage.totalPages || loading}
                  onClick={() => setPage((p) => p + 1)}
                  className="rounded-lg border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {t('common.next')}
                </button>
              </div>
            )}
          </section>
        </>
      )}
    </div>
  );
}
