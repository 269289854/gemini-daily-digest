import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  AlertTriangle,
  Bookmark,
  BookmarkCheck,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  Loader2,
  Radio,
  RefreshCw,
  Search,
} from 'lucide-react';
import { ApiError, type ApiClient } from '../api/client';
import { SafeMarkdown, sanitizeHttpUrl } from '../components/SafeMarkdown';
import { DEFAULTS } from '../constants/routes';
import type { PlatformBridge } from '../platform/bridge';
import type {
  ItemView,
  PagedResult,
  SourceDailyDigestView,
  UserSourceView,
} from '../types/api';
import { AlertPriority, ErrorCode, FetchOutcome, SourceScope } from '../types/enums';

export interface SourcesExplorerPageProps {
  api: ApiClient;
  bridge: PlatformBridge;
  initialSourceId?: string;
}

interface UiErrorState {
  code?: ErrorCode;
  message: string;
  requestId?: string;
}

function toUiError(err: unknown): UiErrorState {
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
  return { message: 'Unexpected error occurred' };
}

function healthDotClass(status: FetchOutcome | null): string {
  switch (status) {
    case FetchOutcome.UPDATED:
      return 'bg-emerald-400';
    case FetchOutcome.NOT_MODIFIED_304:
      return 'bg-sky-400';
    case FetchOutcome.SKIPPED_FRESH:
      return 'bg-slate-400';
    case FetchOutcome.FAILED:
      return 'bg-rose-500';
    default:
      return 'bg-slate-600';
  }
}

function healthStatusLabel(status: FetchOutcome | null): string {
  return status ?? 'never_fetched';
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

function formatDateTime(value: string | null | undefined): string {
  if (!value) {
    return 'Never';
  }
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return value;
  }
  return parsed.toLocaleString();
}

export function SourcesExplorerPage({
  api,
  bridge,
  initialSourceId,
}: SourcesExplorerPageProps) {
  const [sources, setSources] = useState<UserSourceView[]>([]);
  const [loadingSources, setLoadingSources] = useState(true);
  const [selectedSourceId, setSelectedSourceId] = useState<string>(initialSourceId ?? '');

  const [filterText, setFilterText] = useState('');
  const [categoryFilter, setCategoryFilter] = useState('all');
  const [scopeFilter, setScopeFilter] = useState<'all' | SourceScope>('all');

  const [digestsPage, setDigestsPage] = useState(1);
  const [sourceDigests, setSourceDigests] = useState<PagedResult<SourceDailyDigestView> | null>(
    null,
  );
  const [loadingDigests, setLoadingDigests] = useState(false);

  const [itemsPage, setItemsPage] = useState(1);
  const [sourceItems, setSourceItems] = useState<PagedResult<ItemView> | null>(null);
  const [loadingItems, setLoadingItems] = useState(false);

  const [bookmarkBusyId, setBookmarkBusyId] = useState<string | null>(null);
  const [error, setError] = useState<UiErrorState | null>(null);

  useEffect(() => {
    if (initialSourceId) {
      setSelectedSourceId(initialSourceId);
      setDigestsPage(1);
      setItemsPage(1);
    }
  }, [initialSourceId]);

  const loadSources = useCallback(async () => {
    setLoadingSources(true);
    setError(null);
    try {
      const list = await api.listSources();
      setSources(list);
      setSelectedSourceId((current) => {
        if (current && list.some((s) => s.id === current)) {
          return current;
        }
        if (initialSourceId && list.some((s) => s.id === initialSourceId)) {
          return initialSourceId;
        }
        return list.length > 0 ? list[0].id : '';
      });
    } catch (err) {
      setError(toUiError(err));
    } finally {
      setLoadingSources(false);
    }
  }, [api, initialSourceId]);

  useEffect(() => {
    void loadSources();
  }, [loadSources]);

  const loadSourceDigests = useCallback(
    async (sourceId: string, page: number) => {
      if (!sourceId) {
        setSourceDigests(null);
        return;
      }
      setLoadingDigests(true);
      try {
        const res = await api.getSourceDigests(sourceId, { page, pageSize: DEFAULTS.PAGE_SIZE });
        setSourceDigests(res);
      } catch (err) {
        setError(toUiError(err));
      } finally {
        setLoadingDigests(false);
      }
    },
    [api],
  );

  const loadSourceItems = useCallback(
    async (sourceId: string, page: number) => {
      if (!sourceId) {
        setSourceItems(null);
        return;
      }
      setLoadingItems(true);
      try {
        const res = await api.getSourceItems(sourceId, { page, pageSize: DEFAULTS.PAGE_SIZE });
        setSourceItems(res);
      } catch (err) {
        setError(toUiError(err));
      } finally {
        setLoadingItems(false);
      }
    },
    [api],
  );

  useEffect(() => {
    if (selectedSourceId) {
      void loadSourceDigests(selectedSourceId, digestsPage);
    }
  }, [selectedSourceId, digestsPage, loadSourceDigests]);

  useEffect(() => {
    if (selectedSourceId) {
      void loadSourceItems(selectedSourceId, itemsPage);
    }
  }, [selectedSourceId, itemsPage, loadSourceItems]);

  const categories = useMemo(() => {
    const set = new Set<string>();
    for (const s of sources) {
      if (s.category) {
        set.add(s.category);
      }
    }
    return Array.from(set).sort();
  }, [sources]);

  const filteredSources = useMemo(() => {
    const q = filterText.trim().toLowerCase();
    return sources.filter((source) => {
      if (categoryFilter !== 'all' && source.category !== categoryFilter) {
        return false;
      }
      if (scopeFilter !== 'all' && source.scope !== scopeFilter) {
        return false;
      }
      if (!q) {
        return true;
      }
      return (
        source.customName.toLowerCase().includes(q) ||
        source.defaultName.toLowerCase().includes(q) ||
        source.handleOrUrl.toLowerCase().includes(q) ||
        source.category.toLowerCase().includes(q) ||
        source.sourceType.toLowerCase().includes(q)
      );
    });
  }, [sources, filterText, categoryFilter, scopeFilter]);

  const selectedSource = useMemo(
    () => sources.find((s) => s.id === selectedSourceId) ?? null,
    [sources, selectedSourceId],
  );

  const handleSelectSource = (sourceId: string) => {
    setSelectedSourceId(sourceId);
    setDigestsPage(1);
    setItemsPage(1);
    setError(null);
  };

  const handleOpenExternal = (rawUrl: string) => {
    const safe = sanitizeHttpUrl(rawUrl);
    if (!safe) {
      return;
    }
    void bridge.openExternalUrl(safe);
  };

  const handleToggleBookmark = async (item: ItemView) => {
    setBookmarkBusyId(item.id);
    setError(null);
    try {
      if (item.bookmarked) {
        await api.deleteBookmark(item.id);
        setSourceItems((prev) =>
          prev
            ? {
                ...prev,
                items: prev.items.map((it) =>
                  it.id === item.id ? { ...it, bookmarked: false } : it,
                ),
              }
            : prev,
        );
      } else {
        await api.upsertBookmark({ itemId: item.id, note: item.bookmarkNote ?? undefined });
        setSourceItems((prev) =>
          prev
            ? {
                ...prev,
                items: prev.items.map((it) =>
                  it.id === item.id ? { ...it, bookmarked: true } : it,
                ),
              }
            : prev,
        );
      }
    } catch (err) {
      setError(toUiError(err));
    } finally {
      setBookmarkBusyId(null);
    }
  };

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-xl font-bold tracking-tight text-white">Sources Explorer</h1>
          <p className="text-xs text-slate-400">
            Inspect per-account health, daily independent summary timelines, and raw collected items.
          </p>
        </div>
        <button
          type="button"
          onClick={() => {
            void loadSources();
            if (selectedSourceId) {
              void loadSourceDigests(selectedSourceId, digestsPage);
              void loadSourceItems(selectedSourceId, itemsPage);
            }
          }}
          className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-800"
        >
          <RefreshCw className="h-3.5 w-3.5" />
          Refresh
        </button>
      </div>

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
                  Request ID: {error.requestId}
                </div>
              )}
            </div>
          </div>
          <button
            type="button"
            onClick={() => setError(null)}
            className="text-xs text-rose-300 hover:text-white"
          >
            Dismiss
          </button>
        </div>
      )}

      <div className="grid gap-5 lg:grid-cols-12">
        {/* Left Panel: Subscribed Sources List */}
        <aside className="space-y-3 rounded-xl border border-slate-800 bg-slate-900/70 p-4 lg:col-span-4">
          <div className="space-y-2.5">
            <div className="relative">
              <Search className="pointer-events-none absolute left-2.5 top-2.5 h-3.5 w-3.5 text-slate-500" />
              <input
                type="text"
                aria-label="Filter sources"
                placeholder="Filter sources by name or handle..."
                value={filterText}
                onChange={(event) => setFilterText(event.target.value)}
                className="w-full rounded-lg border border-slate-700 bg-slate-950/80 py-1.5 pl-8 pr-3 text-xs text-slate-100 placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
              />
            </div>

            <div className="grid grid-cols-2 gap-2">
              <select
                aria-label="Filter by category"
                value={categoryFilter}
                onChange={(event) => setCategoryFilter(event.target.value)}
                className="rounded-lg border border-slate-700 bg-slate-950/80 px-2.5 py-1.5 text-xs text-slate-200 focus:border-indigo-500 focus:outline-none"
              >
                <option value="all">All categories</option>
                {categories.map((cat) => (
                  <option key={cat} value={cat}>
                    {cat}
                  </option>
                ))}
              </select>

              <select
                aria-label="Filter by scope"
                value={scopeFilter}
                onChange={(event) => setScopeFilter(event.target.value as 'all' | SourceScope)}
                className="rounded-lg border border-slate-700 bg-slate-950/80 px-2.5 py-1.5 text-xs text-slate-200 focus:border-indigo-500 focus:outline-none"
              >
                <option value="all">All scopes</option>
                <option value={SourceScope.SHARED_PUBLIC}>{SourceScope.SHARED_PUBLIC}</option>
                <option value={SourceScope.USER_PRIVATE}>{SourceScope.USER_PRIVATE}</option>
              </select>
            </div>
          </div>

          {loadingSources ? (
            <div className="flex items-center justify-center gap-2 py-8 text-xs text-slate-400">
              <Loader2 className="h-4 w-4 animate-spin text-indigo-400" />
              Loading sources...
            </div>
          ) : filteredSources.length === 0 ? (
            <div className="rounded-lg border border-dashed border-slate-800 p-6 text-center text-xs text-slate-400">
              No matching sources found.
            </div>
          ) : (
            <ul className="max-h-[70vh] space-y-2 overflow-y-auto pr-1">
              {filteredSources.map((source) => {
                const selected = source.id === selectedSourceId;
                return (
                  <li key={source.id}>
                    <button
                      type="button"
                      onClick={() => handleSelectSource(source.id)}
                      title={source.lastError ?? undefined}
                      className={`w-full rounded-lg border p-3 text-left transition-colors ${
                        selected
                          ? 'border-indigo-500 bg-indigo-950/40'
                          : 'border-slate-800 bg-slate-950/50 hover:border-slate-700 hover:bg-slate-900/70'
                      }`}
                    >
                      <div className="flex items-start justify-between gap-2">
                        <div className="flex min-w-0 items-center gap-2">
                          <span
                            className={`h-2.5 w-2.5 shrink-0 rounded-full ${healthDotClass(
                              source.lastStatus,
                            )}`}
                            title={`Status: ${healthStatusLabel(source.lastStatus)}`}
                            aria-label={`Health status: ${healthStatusLabel(source.lastStatus)}`}
                          />
                          <span className="truncate text-sm font-medium text-white">
                            {source.customName || source.defaultName}
                          </span>
                        </div>
                        {source.consecutiveFailures > 0 && (
                          <span className="shrink-0 rounded-full bg-rose-500/20 px-2 py-0.5 text-[10px] font-semibold text-rose-300">
                            {source.consecutiveFailures} failures
                          </span>
                        )}
                      </div>

                      <div className="mt-1.5 flex flex-wrap items-center gap-1.5 text-[11px] text-slate-400">
                        <span className="rounded bg-slate-800 px-1.5 py-0.5 text-slate-300">
                          {source.category}
                        </span>
                        <span className="font-mono text-slate-400">{source.sourceType}</span>
                        {!source.enabled && (
                          <span className="rounded bg-slate-800 px-1.5 py-0.5 text-amber-300">
                            Paused
                          </span>
                        )}
                      </div>

                      <div className="mt-1.5 text-[11px] text-slate-500">
                        Last fetch: {formatDateTime(source.lastFetchedAt)}
                      </div>

                      {source.lastError && (
                        <p
                          className="mt-1.5 truncate rounded bg-rose-950/40 px-2 py-1 text-[11px] text-rose-300"
                          title={source.lastError}
                        >
                          {source.lastError}
                        </p>
                      )}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </aside>

        {/* Right Panel: Selected Source Details, Daily Timeline, Raw Items */}
        <section className="space-y-5 lg:col-span-8">
          {!selectedSource ? (
            <div className="flex flex-col items-center justify-center rounded-xl border border-dashed border-slate-800 bg-slate-900/40 py-16 text-center text-xs text-slate-400">
              <Radio className="mb-2 h-7 w-7 text-slate-600" />
              Select a source from the left panel to view its daily digest timeline and collected
              items.
            </div>
          ) : (
            <>
              {/* Source Metadata Header */}
              <div className="rounded-xl border border-slate-800 bg-slate-900/70 p-4 sm:p-5">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="space-y-1">
                    <div className="flex items-center gap-2">
                      <span
                        className={`h-2.5 w-2.5 rounded-full ${healthDotClass(
                          selectedSource.lastStatus,
                        )}`}
                      />
                      <h2 className="text-lg font-bold text-white">
                        {selectedSource.customName || selectedSource.defaultName}
                      </h2>
                    </div>
                    <p className="font-mono text-xs text-slate-400">
                      {selectedSource.handleOrUrl}
                    </p>
                  </div>

                  <div className="flex flex-wrap gap-2 text-xs">
                    <span className="rounded-full border border-slate-700 bg-slate-800 px-2.5 py-0.5 text-slate-300">
                      {selectedSource.sourceType}
                    </span>
                    <span className="rounded-full border border-slate-700 bg-slate-800 px-2.5 py-0.5 text-slate-300">
                      {selectedSource.scope}
                    </span>
                    <span className="rounded-full border border-slate-700 bg-slate-800 px-2.5 py-0.5 text-slate-300">
                      {healthStatusLabel(selectedSource.lastStatus)}
                    </span>
                  </div>
                </div>

                {selectedSource.lastError && (
                  <div className="mt-3 rounded-lg border border-rose-500/30 bg-rose-950/30 px-3 py-2 text-xs text-rose-300">
                    Last Error: {selectedSource.lastError}
                  </div>
                )}
              </div>

              {/* Per-Source Daily Independent Summary Timeline */}
              <div className="rounded-xl border border-slate-800 bg-slate-900/70 p-5">
                <div className="flex items-center justify-between gap-2 border-b border-slate-800 pb-3">
                  <h3 className="text-sm font-semibold text-white">
                    Daily Independent Summary Timeline
                  </h3>
                  {sourceDigests && (
                    <span className="text-xs text-slate-400">
                      {sourceDigests.total} digests
                    </span>
                  )}
                </div>

                {loadingDigests ? (
                  <div className="flex items-center justify-center gap-2 py-8 text-xs text-slate-400">
                    <Loader2 className="h-4 w-4 animate-spin text-indigo-400" />
                    Loading source summaries...
                  </div>
                ) : !sourceDigests || sourceDigests.items.length === 0 ? (
                  <p className="py-6 text-center text-xs text-slate-400">
                    No daily summaries recorded for this source yet.
                  </p>
                ) : (
                  <div className="mt-4 space-y-4">
                    {sourceDigests.items.map((entry) => (
                      <article
                        key={entry.id}
                        className="rounded-lg border border-slate-800 bg-slate-950/60 p-4"
                      >
                        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-800/70 pb-2">
                          <span className="rounded bg-indigo-500/15 px-2 py-0.5 text-xs font-semibold text-indigo-300">
                            {entry.digestDate}
                          </span>
                          <span className="text-xs text-slate-400">
                            {entry.itemCount} items summarized
                          </span>
                        </div>

                        <div className="mt-3">
                          <SafeMarkdown content={entry.summaryMarkdown} bridge={bridge} />
                        </div>

                        {entry.keyPoints.length > 0 && (
                          <div className="mt-3 flex flex-wrap gap-1.5">
                            {entry.keyPoints.map((kp, idx) => (
                              <span
                                key={idx}
                                className="rounded-md border border-slate-800 bg-slate-900 px-2 py-1 text-xs text-slate-300"
                              >
                                {kp}
                              </span>
                            ))}
                          </div>
                        )}
                      </article>
                    ))}

                    {sourceDigests.totalPages > 1 && (
                      <div className="flex items-center justify-between pt-2 text-xs text-slate-400">
                        <span>
                          Page {sourceDigests.page} of {sourceDigests.totalPages}
                        </span>
                        <div className="flex items-center gap-2">
                          <button
                            type="button"
                            disabled={sourceDigests.page <= 1}
                            onClick={() => setDigestsPage((p) => Math.max(1, p - 1))}
                            className="inline-flex items-center gap-1 rounded border border-slate-700 bg-slate-900 px-2.5 py-1 text-slate-200 hover:bg-slate-800 disabled:opacity-40"
                          >
                            <ChevronLeft className="h-3.5 w-3.5" />
                            Prev
                          </button>
                          <button
                            type="button"
                            disabled={sourceDigests.page >= sourceDigests.totalPages}
                            onClick={() => setDigestsPage((p) => p + 1)}
                            className="inline-flex items-center gap-1 rounded border border-slate-700 bg-slate-900 px-2.5 py-1 text-slate-200 hover:bg-slate-800 disabled:opacity-40"
                          >
                            Next
                            <ChevronRight className="h-3.5 w-3.5" />
                          </button>
                        </div>
                      </div>
                    )}
                  </div>
                )}
              </div>

              {/* Paginated Raw Collected Items Stream */}
              <div className="rounded-xl border border-slate-800 bg-slate-900/70 p-5">
                <div className="flex items-center justify-between gap-2 border-b border-slate-800 pb-3">
                  <h3 className="text-sm font-semibold text-white">Collected Items Stream</h3>
                  {sourceItems && (
                    <span className="text-xs text-slate-400">{sourceItems.total} items</span>
                  )}
                </div>

                {loadingItems ? (
                  <div className="flex items-center justify-center gap-2 py-8 text-xs text-slate-400">
                    <Loader2 className="h-4 w-4 animate-spin text-indigo-400" />
                    Loading collected items...
                  </div>
                ) : !sourceItems || sourceItems.items.length === 0 ? (
                  <p className="py-6 text-center text-xs text-slate-400">
                    No items collected from this source yet.
                  </p>
                ) : (
                  <div className="mt-4 space-y-3">
                    {sourceItems.items.map((item) => {
                      const safeUrl = sanitizeHttpUrl(item.canonicalUrl);
                      const busy = bookmarkBusyId === item.id;
                      return (
                        <article
                          key={item.id}
                          className="rounded-lg border border-slate-800 bg-slate-950/60 p-4"
                        >
                          <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
                            <div className="space-y-1.5">
                              <div className="flex flex-wrap items-center gap-2">
                                <span
                                  className={`rounded-full border px-2 py-0.5 text-[10px] font-semibold uppercase ${priorityBadgeClass(
                                    item.priority,
                                  )}`}
                                >
                                  {item.priority}
                                </span>
                                <span className="text-[11px] text-slate-400">
                                  Score: {item.importanceScore}
                                </span>
                                {item.author && (
                                  <span className="text-[11px] text-slate-400">
                                    by {item.author}
                                  </span>
                                )}
                                <span className="text-[11px] text-slate-500">
                                  {formatDateTime(item.publishedAt ?? item.fetchedAt)}
                                </span>
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

                              {item.bodyPruned ? (
                                <span className="inline-block rounded border border-slate-700 bg-slate-900 px-2 py-0.5 text-[11px] text-slate-400">
                                  Body pruned (&gt;180 days)
                                </span>
                              ) : (
                                item.summary && (
                                  <p className="text-xs leading-relaxed text-slate-400">
                                    {item.summary}
                                  </p>
                                )
                              )}

                              {item.matchedKeywords.length > 0 && (
                                <div className="flex flex-wrap gap-1">
                                  {item.matchedKeywords.map((kw) => (
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

                            <button
                              type="button"
                              onClick={() => void handleToggleBookmark(item)}
                              disabled={busy}
                              aria-label={item.bookmarked ? 'Remove bookmark' : 'Bookmark item'}
                              className={`inline-flex shrink-0 items-center gap-1 rounded-lg border px-2.5 py-1 text-xs font-medium transition-colors ${
                                item.bookmarked
                                  ? 'border-amber-500/50 bg-amber-500/15 text-amber-300 hover:bg-amber-500/25'
                                  : 'border-slate-700 bg-slate-900 text-slate-300 hover:bg-slate-800'
                              } disabled:opacity-50`}
                            >
                              {busy ? (
                                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                              ) : item.bookmarked ? (
                                <BookmarkCheck className="h-3.5 w-3.5" />
                              ) : (
                                <Bookmark className="h-3.5 w-3.5" />
                              )}
                              {item.bookmarked ? 'Bookmarked' : 'Bookmark'}
                            </button>
                          </div>
                        </article>
                      );
                    })}

                    {sourceItems.totalPages > 1 && (
                      <div className="flex items-center justify-between pt-2 text-xs text-slate-400">
                        <span>
                          Page {sourceItems.page} of {sourceItems.totalPages}
                        </span>
                        <div className="flex items-center gap-2">
                          <button
                            type="button"
                            disabled={sourceItems.page <= 1}
                            onClick={() => setItemsPage((p) => Math.max(1, p - 1))}
                            className="inline-flex items-center gap-1 rounded border border-slate-700 bg-slate-900 px-2.5 py-1 text-slate-200 hover:bg-slate-800 disabled:opacity-40"
                          >
                            <ChevronLeft className="h-3.5 w-3.5" />
                            Prev
                          </button>
                          <button
                            type="button"
                            disabled={sourceItems.page >= sourceItems.totalPages}
                            onClick={() => setItemsPage((p) => p + 1)}
                            className="inline-flex items-center gap-1 rounded border border-slate-700 bg-slate-900 px-2.5 py-1 text-slate-200 hover:bg-slate-800 disabled:opacity-40"
                          >
                            Next
                            <ChevronRight className="h-3.5 w-3.5" />
                          </button>
                        </div>
                      </div>
                    )}
                  </div>
                )}
              </div>
            </>
          )}
        </section>
      </div>
    </div>
  );
}

export default SourcesExplorerPage;
