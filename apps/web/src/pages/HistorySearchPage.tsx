import { type FormEvent, useCallback, useEffect, useState } from 'react';
import {
  AlertTriangle,
  Bookmark,
  BookmarkCheck,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  Loader2,
  NotebookPen,
  RotateCcw,
  Save,
  Search,
} from 'lucide-react';
import { ApiError, type ApiClient } from '../api/client';
import { sanitizeHttpUrl } from '../components/SafeMarkdown';
import { DEFAULTS } from '../constants/routes';
import type { PlatformBridge } from '../platform/bridge';
import type {
  ItemSearchParams,
  ItemView,
  PagedResult,
  UserSourceView,
} from '../types/api';
import { AlertPriority, ErrorCode } from '../types/enums';

export interface HistorySearchPageProps {
  api: ApiClient;
  bridge: PlatformBridge;
  initialQuery?: string;
  initialBookmarked?: boolean;
}

interface UiErrorState {
  code?: ErrorCode;
  message: string;
  requestId?: string;
}

interface SearchFilters {
  q: string;
  sourceId: string;
  dateFrom: string;
  dateTo: string;
  keyword: string;
  bookmarkedOnly: boolean;
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
    return 'Unknown time';
  }
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return value;
  }
  return parsed.toLocaleString();
}

export function HistorySearchPage({
  api,
  bridge,
  initialQuery = '',
  initialBookmarked = false,
}: HistorySearchPageProps) {
  const [sources, setSources] = useState<UserSourceView[]>([]);
  const [filters, setFilters] = useState<SearchFilters>({
    q: initialQuery,
    sourceId: '',
    dateFrom: '',
    dateTo: '',
    keyword: '',
    bookmarkedOnly: initialBookmarked,
  });
  const [appliedFilters, setAppliedFilters] = useState<SearchFilters>({
    q: initialQuery,
    sourceId: '',
    dateFrom: '',
    dateTo: '',
    keyword: '',
    bookmarkedOnly: initialBookmarked,
  });
  const [page, setPage] = useState(1);

  const [results, setResults] = useState<PagedResult<ItemView> | null>(null);
  const [loading, setLoading] = useState(false);
  const [dateError, setDateError] = useState<string | null>(null);
  const [error, setError] = useState<UiErrorState | null>(null);

  const [bookmarkBusyId, setBookmarkBusyId] = useState<string | null>(null);
  const [editingNoteItemId, setEditingNoteItemId] = useState<string | null>(null);
  const [noteDraft, setNoteDraft] = useState('');
  const [savingNoteId, setSavingNoteId] = useState<string | null>(null);

  useEffect(() => {
    setFilters((prev) => ({
      ...prev,
      q: initialQuery,
      bookmarkedOnly: initialBookmarked,
    }));
    setAppliedFilters((prev) => ({
      ...prev,
      q: initialQuery,
      bookmarkedOnly: initialBookmarked,
    }));
    setPage(1);
  }, [initialQuery, initialBookmarked]);

  useEffect(() => {
    let cancelled = false;
    async function loadSources() {
      try {
        const list = await api.listSources();
        if (!cancelled) {
          setSources(list);
        }
      } catch {
        // Non-fatal: search still functions without source dropdown options.
      }
    }
    void loadSources();
    return () => {
      cancelled = true;
    };
  }, [api]);

  const executeSearch = useCallback(
    async (activeFilters: SearchFilters, targetPage: number) => {
      if (
        activeFilters.dateFrom &&
        activeFilters.dateTo &&
        activeFilters.dateFrom > activeFilters.dateTo
      ) {
        setDateError('Start date must be on or before end date.');
        return;
      }

      setDateError(null);
      setLoading(true);
      setError(null);
      try {
        const params: ItemSearchParams = {
          q: activeFilters.q.trim() || undefined,
          sourceId: activeFilters.sourceId || undefined,
          dateFrom: activeFilters.dateFrom || undefined,
          dateTo: activeFilters.dateTo || undefined,
          keyword: activeFilters.keyword.trim() || undefined,
          bookmarked: activeFilters.bookmarkedOnly ? true : undefined,
          page: targetPage,
          pageSize: DEFAULTS.PAGE_SIZE,
        };
        const res = await api.searchItems(params);
        setResults(res);
      } catch (err) {
        setError(toUiError(err));
      } finally {
        setLoading(false);
      }
    },
    [api],
  );

  useEffect(() => {
    void executeSearch(appliedFilters, page);
  }, [appliedFilters, page, executeSearch]);

  const validateDates = (from: string, to: string): string | null => {
    if (from && to && from > to) {
      return 'Start date must be on or before end date.';
    }
    return null;
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const validationErr = validateDates(filters.dateFrom, filters.dateTo);
    if (validationErr) {
      setDateError(validationErr);
      return;
    }
    setDateError(null);
    setPage(1);
    setAppliedFilters({ ...filters });
  };

  const handleReset = () => {
    const empty: SearchFilters = {
      q: '',
      sourceId: '',
      dateFrom: '',
      dateTo: '',
      keyword: '',
      bookmarkedOnly: false,
    };
    setFilters(empty);
    setAppliedFilters(empty);
    setDateError(null);
    setPage(1);
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
        setResults((prev) =>
          prev
            ? {
                ...prev,
                items: prev.items
                  .map((it) =>
                    it.id === item.id ? { ...it, bookmarked: false, bookmarkNote: null } : it,
                  )
                  .filter((it) => !(appliedFilters.bookmarkedOnly && !it.bookmarked)),
              }
            : prev,
        );
        if (editingNoteItemId === item.id) {
          setEditingNoteItemId(null);
        }
      } else {
        await api.upsertBookmark({ itemId: item.id, note: item.bookmarkNote ?? undefined });
        setResults((prev) =>
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

  const handleStartEditNote = (item: ItemView) => {
    setEditingNoteItemId(item.id);
    setNoteDraft(item.bookmarkNote ?? '');
  };

  const handleSaveNote = async (item: ItemView) => {
    setSavingNoteId(item.id);
    setError(null);
    try {
      const trimmed = noteDraft.trim();
      const saved = await api.upsertBookmark({
        itemId: item.id,
        note: trimmed ? trimmed : null,
      });
      setResults((prev) =>
        prev
          ? {
              ...prev,
              items: prev.items.map((it) =>
                it.id === item.id
                  ? { ...it, bookmarked: true, bookmarkNote: saved.note }
                  : it,
              ),
            }
          : prev,
      );
      setEditingNoteItemId(null);
    } catch (err) {
      setError(toUiError(err));
    } finally {
      setSavingNoteId(null);
    }
  };

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-xl font-bold tracking-tight text-white">History &amp; Bookmarks</h1>
        <p className="text-xs text-slate-400">
          Full-text search across collected items, filter by source or watchlist keyword, and manage
          bookmark notes.
        </p>
      </div>

      {/* Filter Bar */}
      <form
        onSubmit={handleSubmit}
        className="space-y-3 rounded-xl border border-slate-800 bg-slate-900/70 p-4 sm:p-5"
      >
        <div className="grid gap-3 md:grid-cols-12">
          <div className="relative md:col-span-5">
            <Search className="pointer-events-none absolute left-3 top-2.5 h-4 w-4 text-slate-500" />
            <input
              type="text"
              aria-label="Search query"
              placeholder="Full-text search (title, summary, author)..."
              value={filters.q}
              onChange={(event) => setFilters((prev) => ({ ...prev, q: event.target.value }))}
              className="w-full rounded-lg border border-slate-700 bg-slate-950/80 py-2 pl-9 pr-3 text-xs text-slate-100 placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
            />
          </div>

          <div className="md:col-span-4">
            <select
              aria-label="Filter by source"
              value={filters.sourceId}
              onChange={(event) =>
                setFilters((prev) => ({ ...prev, sourceId: event.target.value }))
              }
              className="w-full rounded-lg border border-slate-700 bg-slate-950/80 px-3 py-2 text-xs text-slate-200 focus:border-indigo-500 focus:outline-none"
            >
              <option value="">All subscribed sources</option>
              {sources.map((source) => (
                <option key={source.id} value={source.id}>
                  {source.customName || source.defaultName} ({source.category})
                </option>
              ))}
            </select>
          </div>

          <div className="md:col-span-3">
            <input
              type="text"
              aria-label="Matched keyword"
              placeholder="Matched keyword..."
              value={filters.keyword}
              onChange={(event) =>
                setFilters((prev) => ({ ...prev, keyword: event.target.value }))
              }
              className="w-full rounded-lg border border-slate-700 bg-slate-950/80 px-3 py-2 text-xs text-slate-100 placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
            />
          </div>
        </div>

        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex flex-wrap items-center gap-3">
            <label className="flex items-center gap-2 text-xs text-slate-300">
              <span>From</span>
              <input
                type="date"
                aria-label="Date from"
                value={filters.dateFrom}
                onChange={(event) => {
                  const nextFrom = event.target.value;
                  setFilters((prev) => ({ ...prev, dateFrom: nextFrom }));
                  setDateError(validateDates(nextFrom, filters.dateTo));
                }}
                className="rounded-lg border border-slate-700 bg-slate-950/80 px-2.5 py-1.5 text-xs text-slate-100 focus:border-indigo-500 focus:outline-none"
              />
            </label>

            <label className="flex items-center gap-2 text-xs text-slate-300">
              <span>To</span>
              <input
                type="date"
                aria-label="Date to"
                value={filters.dateTo}
                onChange={(event) => {
                  const nextTo = event.target.value;
                  setFilters((prev) => ({ ...prev, dateTo: nextTo }));
                  setDateError(validateDates(filters.dateFrom, nextTo));
                }}
                className="rounded-lg border border-slate-700 bg-slate-950/80 px-2.5 py-1.5 text-xs text-slate-100 focus:border-indigo-500 focus:outline-none"
              />
            </label>

            <div className="inline-flex rounded-lg border border-slate-700 bg-slate-950/80 p-0.5 text-xs">
              <button
                type="button"
                onClick={() => {
                  const next = { ...filters, bookmarkedOnly: false };
                  setFilters(next);
                  setPage(1);
                  setAppliedFilters(next);
                }}
                className={`rounded-md px-2.5 py-1 font-medium transition-colors ${
                  !filters.bookmarkedOnly
                    ? 'bg-indigo-600 text-white'
                    : 'text-slate-400 hover:text-slate-200'
                }`}
              >
                All
              </button>
              <button
                type="button"
                onClick={() => {
                  const next = { ...filters, bookmarkedOnly: true };
                  setFilters(next);
                  setPage(1);
                  setAppliedFilters(next);
                }}
                className={`rounded-md px-2.5 py-1 font-medium transition-colors ${
                  filters.bookmarkedOnly
                    ? 'bg-amber-600 text-white'
                    : 'text-slate-400 hover:text-slate-200'
                }`}
              >
                Bookmarked only
              </button>
            </div>
          </div>

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={handleReset}
              className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs font-medium text-slate-300 hover:bg-slate-800"
            >
              <RotateCcw className="h-3.5 w-3.5" />
              Reset
            </button>
            <button
              type="submit"
              disabled={Boolean(dateError) || loading}
              className="inline-flex items-center gap-1.5 rounded-lg bg-indigo-600 px-3.5 py-1.5 text-xs font-semibold text-white hover:bg-indigo-500 disabled:cursor-not-allowed disabled:opacity-60"
            >
              <Search className="h-3.5 w-3.5" />
              Search
            </button>
          </div>
        </div>

        {dateError && (
          <p role="alert" className="text-xs font-medium text-rose-400">
            {dateError}
          </p>
        )}
      </form>

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

      {/* Results Header */}
      {results && (
        <div className="flex items-center justify-between text-xs text-slate-400">
          <span>
            Showing {results.items.length} of {results.total} items
          </span>
          <span>
            Page {results.page} of {Math.max(1, results.totalPages)}
          </span>
        </div>
      )}

      {/* Results List */}
      {loading ? (
        <div className="flex items-center justify-center gap-2 rounded-xl border border-slate-800 bg-slate-900/50 py-14 text-xs text-slate-400">
          <Loader2 className="h-4 w-4 animate-spin text-indigo-400" />
          Searching collected items...
        </div>
      ) : !results || results.items.length === 0 ? (
        <div className="rounded-xl border border-dashed border-slate-800 bg-slate-900/40 py-12 text-center text-xs text-slate-400">
          No items match your current filters.
        </div>
      ) : (
        <div className="space-y-3">
          {results.items.map((item) => {
            const safeUrl = sanitizeHttpUrl(item.canonicalUrl);
            const bookmarkBusy = bookmarkBusyId === item.id;
            const isEditingNote = editingNoteItemId === item.id;
            const isSavingNote = savingNoteId === item.id;

            return (
              <article
                key={item.id}
                className="rounded-xl border border-slate-800 bg-slate-900/70 p-4 sm:p-5"
              >
                <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                  <div className="space-y-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="rounded bg-indigo-500/15 px-2 py-0.5 text-[11px] font-semibold text-indigo-300">
                        {item.sourceName}
                      </span>
                      <span className="rounded bg-slate-800 px-2 py-0.5 text-[11px] text-slate-300">
                        {item.category}
                      </span>
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
                        <span className="text-[11px] text-slate-400">by {item.author}</span>
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
                        className="inline-flex items-center gap-1.5 text-base font-semibold text-white hover:text-indigo-300"
                      >
                        <span>{item.title}</span>
                        <ExternalLink className="h-3.5 w-3.5 shrink-0 text-indigo-400" />
                      </a>
                    ) : (
                      <h2 className="text-base font-semibold text-white">{item.title}</h2>
                    )}

                    {item.bodyPruned ? (
                      <span className="inline-block rounded border border-slate-700 bg-slate-950 px-2 py-0.5 text-[11px] text-slate-400">
                        Body pruned (&gt;180 days)
                      </span>
                    ) : (
                      item.summary && (
                        <p className="text-xs leading-relaxed text-slate-300">{item.summary}</p>
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

                    {/* Saved Bookmark Note Display */}
                    {item.bookmarkNote && !isEditingNote && (
                      <div className="rounded-lg border border-amber-500/30 bg-amber-950/25 px-3 py-2 text-xs text-amber-200">
                        <span className="font-semibold text-amber-300">Note: </span>
                        {item.bookmarkNote}
                      </div>
                    )}

                    {/* Inline Personal Note Editor */}
                    {isEditingNote && (
                      <div className="space-y-2 rounded-lg border border-slate-700 bg-slate-950/80 p-3">
                        <label className="block text-[11px] font-medium text-slate-300">
                          Personal Note
                          <textarea
                            rows={2}
                            value={noteDraft}
                            onChange={(event) => setNoteDraft(event.target.value)}
                            placeholder="Write a personal note for this item..."
                            className="mt-1 w-full rounded-md border border-slate-700 bg-slate-900 p-2 text-xs text-slate-100 placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
                          />
                        </label>
                        <div className="flex items-center justify-end gap-2">
                          <button
                            type="button"
                            onClick={() => setEditingNoteItemId(null)}
                            className="rounded-md border border-slate-700 px-2.5 py-1 text-xs text-slate-300 hover:bg-slate-800"
                          >
                            Cancel
                          </button>
                          <button
                            type="button"
                            disabled={isSavingNote}
                            onClick={() => void handleSaveNote(item)}
                            className="inline-flex items-center gap-1 rounded-md bg-indigo-600 px-2.5 py-1 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-60"
                          >
                            {isSavingNote ? (
                              <Loader2 className="h-3 w-3 animate-spin" />
                            ) : (
                              <Save className="h-3 w-3" />
                            )}
                            Save Note
                          </button>
                        </div>
                      </div>
                    )}
                  </div>

                  {/* Actions */}
                  <div className="flex shrink-0 flex-wrap items-center gap-2 sm:flex-col sm:items-end">
                    <button
                      type="button"
                      onClick={() => void handleToggleBookmark(item)}
                      disabled={bookmarkBusy}
                      aria-label={item.bookmarked ? 'Remove bookmark' : 'Bookmark item'}
                      className={`inline-flex items-center gap-1 rounded-lg border px-2.5 py-1.5 text-xs font-medium transition-colors ${
                        item.bookmarked
                          ? 'border-amber-500/50 bg-amber-500/15 text-amber-300 hover:bg-amber-500/25'
                          : 'border-slate-700 bg-slate-950 text-slate-300 hover:bg-slate-800'
                      } disabled:opacity-50`}
                    >
                      {bookmarkBusy ? (
                        <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      ) : item.bookmarked ? (
                        <BookmarkCheck className="h-3.5 w-3.5" />
                      ) : (
                        <Bookmark className="h-3.5 w-3.5" />
                      )}
                      {item.bookmarked ? 'Bookmarked' : 'Bookmark'}
                    </button>

                    <button
                      type="button"
                      onClick={() => handleStartEditNote(item)}
                      className="inline-flex items-center gap-1 rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs font-medium text-slate-300 hover:bg-slate-800"
                    >
                      <NotebookPen className="h-3.5 w-3.5" />
                      {item.bookmarkNote ? 'Edit Note' : 'Add Note'}
                    </button>
                  </div>
                </div>
              </article>
            );
          })}
        </div>
      )}

      {/* Pagination Controls */}
      {results && results.totalPages > 1 && (
        <div className="flex items-center justify-between rounded-xl border border-slate-800 bg-slate-900/70 px-4 py-3 text-xs text-slate-400">
          <span>
            Page {results.page} of {results.totalPages} ({results.total} total)
          </span>
          <div className="flex items-center gap-2">
            <button
              type="button"
              disabled={results.page <= 1 || loading}
              onClick={() => setPage((p) => Math.max(1, p - 1))}
              className="inline-flex items-center gap-1 rounded-lg border border-slate-700 bg-slate-950 px-3 py-1.5 text-slate-200 hover:bg-slate-800 disabled:opacity-40"
            >
              <ChevronLeft className="h-3.5 w-3.5" />
              Previous
            </button>
            <button
              type="button"
              disabled={results.page >= results.totalPages || loading}
              onClick={() => setPage((p) => p + 1)}
              className="inline-flex items-center gap-1 rounded-lg border border-slate-700 bg-slate-950 px-3 py-1.5 text-slate-200 hover:bg-slate-800 disabled:opacity-40"
            >
              Next
              <ChevronRight className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

export default HistorySearchPage;
