import { type ChangeEvent, type FormEvent, useCallback, useEffect, useState } from 'react';
import {
  CheckCircle2,
  Download,
  Edit3,
  FlaskConical,
  Lock,
  PackagePlus,
  Plus,
  Trash2,
  Upload,
  XCircle,
} from 'lucide-react';
import { ApiError, type ApiClient } from '../../api/client';
import { DEFAULTS } from '../../constants/routes';
import type {
  CreateSourceCmd,
  SourceCredentialsPayload,
  SourceExportBundle,
  SourceTestResult,
  UpdateSourceCmd,
  UserSourceView,
} from '../../types/api';
import { FetchOutcome, SourceScope, SourceType } from '../../types/enums';

export interface SourcesPanelProps {
  api: ApiClient;
}

const SOURCE_TYPE_OPTIONS: { value: SourceType; label: string }[] = [
  { value: SourceType.RSS, label: 'RSS / Atom Feed' },
  { value: SourceType.GITHUB_RELEASE, label: 'GitHub Releases' },
  { value: SourceType.REDDIT, label: 'Reddit Subreddit' },
  { value: SourceType.HACKER_NEWS, label: 'Hacker News' },
  { value: SourceType.GOOGLE_NEWS, label: 'Google News' },
  { value: SourceType.X_ACCOUNT, label: 'X Account (Private)' },
  { value: SourceType.X_SEARCH, label: 'X Search (Private)' },
];

function isXSourceType(sourceType: SourceType): boolean {
  return sourceType === SourceType.X_ACCOUNT || sourceType === SourceType.X_SEARCH;
}

function formatError(err: unknown): string {
  if (err instanceof ApiError) {
    return err.requestId ? `${err.message} (Request ID: ${err.requestId})` : err.message;
  }
  if (err instanceof Error) {
    return err.message;
  }
  return 'Unexpected error';
}

function getOutcomeBadgeClass(outcome: FetchOutcome | null): string {
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

export function SourcesPanel({ api }: SourcesPanelProps) {
  const [sources, setSources] = useState<UserSourceView[]>([]);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);

  // Add Source Form
  const [newSourceType, setNewSourceType] = useState<SourceType>(SourceType.RSS);
  const [newScope, setNewScope] = useState<SourceScope>(SourceScope.SHARED_PUBLIC);
  const [newCustomName, setNewCustomName] = useState<string>('');
  const [newCategory, setNewCategory] = useState<string>('Custom');
  const [newHandleOrUrl, setNewHandleOrUrl] = useState<string>('');
  const [newBearerToken, setNewBearerToken] = useState<string>('');
  const [newApiBaseUrl, setNewApiBaseUrl] = useState<string>('');
  const [creating, setCreating] = useState<boolean>(false);

  // Preset Pack
  const [includeXTemplates, setIncludeXTemplates] = useState<boolean>(false);
  const [importingPreset, setImportingPreset] = useState<boolean>(false);

  // Edit Source State
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editCustomName, setEditCustomName] = useState<string>('');
  const [editCategory, setEditCategory] = useState<string>('');
  const [editHandleOrUrl, setEditHandleOrUrl] = useState<string>('');
  const [editBearerToken, setEditBearerToken] = useState<string>('');
  const [editApiBaseUrl, setEditApiBaseUrl] = useState<string>('');
  const [editClearCredentials, setEditClearCredentials] = useState<boolean>(false);
  const [savingEdit, setSavingEdit] = useState<boolean>(false);

  // Per-source operations
  const [busyIds, setBusyIds] = useState<Record<string, boolean>>({});
  const [testResults, setTestResults] = useState<Record<string, SourceTestResult>>({});

  // Export / Import JSON
  const [exportedJson, setExportedJson] = useState<string>('');
  const [importJson, setImportJson] = useState<string>('');
  const [exporting, setExporting] = useState<boolean>(false);
  const [importingJson, setImportingJson] = useState<boolean>(false);

  const loadSources = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await api.listSources();
      setSources(list);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setLoading(false);
    }
  }, [api]);

  useEffect(() => {
    void loadSources();
  }, [loadSources]);

  const hasNewCredentials =
    newBearerToken.trim().length > 0 || newApiBaseUrl.trim().length > 0;
  const scopeLockedToPrivate = isXSourceType(newSourceType) || hasNewCredentials;
  const effectiveNewScope: SourceScope = scopeLockedToPrivate
    ? SourceScope.USER_PRIVATE
    : newScope;

  const handleAddSource = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    setStatusMessage(null);

    if (!newHandleOrUrl.trim()) {
      setError('Handle or URL is required.');
      return;
    }

    const credentials: SourceCredentialsPayload = {};
    if (newBearerToken.trim()) {
      credentials.bearerToken = newBearerToken.trim();
    }
    if (newApiBaseUrl.trim()) {
      credentials.apiBaseUrl = newApiBaseUrl.trim();
    }

    const cmd: CreateSourceCmd = {
      sourceType: newSourceType,
      scope: effectiveNewScope,
      customName: newCustomName.trim() || undefined,
      category: newCategory.trim() || undefined,
      handleOrUrl: newHandleOrUrl.trim(),
      credentials: Object.keys(credentials).length > 0 ? credentials : undefined,
      enabled: true,
    };

    setCreating(true);
    try {
      const created = await api.createSource(cmd);
      setSources((prev) => {
        const exists = prev.some((s) => s.id === created.id);
        return exists ? prev.map((s) => (s.id === created.id ? created : s)) : [created, ...prev];
      });
      setNewCustomName('');
      setNewHandleOrUrl('');
      setNewBearerToken('');
      setNewApiBaseUrl('');
      setStatusMessage(`Subscribed to source "${created.customName}".`);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setCreating(false);
    }
  };

  const handleImportPreset = async () => {
    setError(null);
    setStatusMessage(null);
    setImportingPreset(true);
    try {
      const res = await api.importPresetSources({ includeXTemplates });
      await loadSources();
      setStatusMessage(`Imported ${res.importedCount} preset sources.`);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setImportingPreset(false);
    }
  };

  const handleToggleEnabled = async (source: UserSourceView) => {
    setError(null);
    setBusyIds((prev) => ({ ...prev, [source.id]: true }));
    try {
      const updated = await api.updateSource(source.id, { enabled: !source.enabled });
      setSources((prev) => prev.map((s) => (s.id === updated.id ? updated : s)));
    } catch (err) {
      setError(formatError(err));
    } finally {
      setBusyIds((prev) => ({ ...prev, [source.id]: false }));
    }
  };

  const startEdit = (source: UserSourceView) => {
    setEditingId(source.id);
    setEditCustomName(source.customName);
    setEditCategory(source.category);
    setEditHandleOrUrl(source.handleOrUrl);
    setEditBearerToken('');
    setEditApiBaseUrl('');
    setEditClearCredentials(false);
  };

  const handleSaveEdit = async (source: UserSourceView) => {
    setError(null);
    setStatusMessage(null);

    if (!editCustomName.trim() || !editCategory.trim()) {
      setError('Custom Name and Category are required.');
      return;
    }

    const cmd: UpdateSourceCmd = {
      customName: editCustomName.trim(),
      category: editCategory.trim(),
    };

    if (source.scope === SourceScope.USER_PRIVATE) {
      if (!editHandleOrUrl.trim()) {
        setError('Handle or URL cannot be empty.');
        return;
      }
      if (editHandleOrUrl.trim() !== source.handleOrUrl) {
        cmd.handleOrUrl = editHandleOrUrl.trim();
      }
      if (editClearCredentials) {
        cmd.clearCredentials = true;
      } else {
        const creds: SourceCredentialsPayload = {};
        if (editBearerToken.trim()) {
          creds.bearerToken = editBearerToken.trim();
        }
        if (editApiBaseUrl.trim()) {
          creds.apiBaseUrl = editApiBaseUrl.trim();
        }
        if (Object.keys(creds).length > 0) {
          cmd.credentials = creds;
        }
      }
    }

    setSavingEdit(true);
    try {
      const updated = await api.updateSource(source.id, cmd);
      setSources((prev) => prev.map((s) => (s.id === updated.id ? updated : s)));
      setEditingId(null);
      setStatusMessage(`Updated source "${updated.customName}".`);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setSavingEdit(false);
    }
  };

  const handleDelete = async (source: UserSourceView) => {
    setError(null);
    setStatusMessage(null);
    setBusyIds((prev) => ({ ...prev, [source.id]: true }));
    try {
      await api.deleteSource(source.id);
      setSources((prev) => prev.filter((s) => s.id !== source.id));
      if (editingId === source.id) {
        setEditingId(null);
      }
      setStatusMessage(`Removed source "${source.customName}".`);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setBusyIds((prev) => ({ ...prev, [source.id]: false }));
    }
  };

  const handleTestSource = async (source: UserSourceView) => {
    setError(null);
    setBusyIds((prev) => ({ ...prev, [source.id]: true }));
    try {
      const result = await api.testSource(source.id);
      setTestResults((prev) => ({ ...prev, [source.id]: result }));
    } catch (err) {
      setError(formatError(err));
    } finally {
      setBusyIds((prev) => ({ ...prev, [source.id]: false }));
    }
  };

  const handleExport = async () => {
    setError(null);
    setStatusMessage(null);
    setExporting(true);
    try {
      const bundle = await api.exportSources();
      const pretty = JSON.stringify(bundle, null, 2);
      setExportedJson(pretty);
      setStatusMessage(`Exported ${bundle.sources.length} non-secret source definitions.`);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setExporting(false);
    }
  };

  const handleDownloadExport = () => {
    if (!exportedJson) {
      return;
    }
    const blob = new Blob([exportedJson], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = 'gemini-digest-sources.json';
    link.click();
    URL.revokeObjectURL(url);
  };

  const handleImportFile = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (!file) {
      return;
    }
    try {
      const text = await file.text();
      setImportJson(text);
    } catch (err) {
      setError(formatError(err));
    }
  };

  const handleImportJson = async () => {
    setError(null);
    setStatusMessage(null);
    if (!importJson.trim()) {
      setError('Paste or upload a valid source export JSON bundle first.');
      return;
    }
    let parsed: SourceExportBundle;
    try {
      parsed = JSON.parse(importJson) as SourceExportBundle;
    } catch {
      setError('Invalid JSON syntax in source import bundle.');
      return;
    }
    if (
      typeof parsed !== 'object' ||
      parsed === null ||
      typeof parsed.version !== 'number' ||
      !Array.isArray(parsed.sources)
    ) {
      setError('Source bundle must include numeric "version" and "sources" array.');
      return;
    }

    setImportingJson(true);
    try {
      const res = await api.importSources(parsed);
      await loadSources();
      setImportJson('');
      setStatusMessage(`Imported ${res.importedCount} sources from JSON bundle.`);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setImportingJson(false);
    }
  };

  return (
    <div className="space-y-6">
      {error && (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-lg border border-rose-500/40 bg-rose-950/50 px-4 py-3 text-sm text-rose-200"
        >
          <XCircle className="mt-0.5 h-4 w-4 shrink-0 text-rose-400" />
          <span>{error}</span>
        </div>
      )}

      {statusMessage && (
        <div
          role="status"
          className="flex items-start gap-2 rounded-lg border border-emerald-500/40 bg-emerald-950/40 px-4 py-3 text-sm text-emerald-200"
        >
          <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-emerald-400" />
          <span>{statusMessage}</span>
        </div>
      )}

      {/* Gemini Ecosystem Preset Pack */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <h2 className="text-base font-semibold text-white">{DEFAULTS.PRESET_PACK_NAME}</h2>
            <p className="mt-1 text-xs text-slate-400">
              Subscribe in one click to curated Gemini blogs, GitHub releases, Reddit, HN, and
              news feeds.
            </p>
            <label className="mt-3 inline-flex items-center gap-2 text-xs text-slate-300">
              <input
                type="checkbox"
                checked={includeXTemplates}
                onChange={(e) => setIncludeXTemplates(e.target.checked)}
                className="h-4 w-4 rounded border-slate-600 bg-slate-900 text-indigo-500"
              />
              <span>Include X account templates (private scope)</span>
            </label>
          </div>
          <button
            type="button"
            disabled={importingPreset}
            onClick={() => void handleImportPreset()}
            className="inline-flex items-center gap-2 rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500 disabled:cursor-not-allowed disabled:opacity-50"
          >
            <PackagePlus className="h-4 w-4" />
            {importingPreset ? 'Importing...' : 'Import Preset Pack'}
          </button>
        </div>
      </section>

      {/* Add Source Form */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-4">
        <div>
          <h2 className="text-base font-semibold text-white">Add Source or Account</h2>
          <p className="mt-0.5 text-xs text-slate-400">
            Public feeds are fetched once and shared across subscribers. Sources with credentials
            or X templates are isolated under <span className="font-mono">user_private</span> scope.
          </p>
        </div>

        <form onSubmit={(e) => void handleAddSource(e)} className="space-y-4">
          <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">Source Type</span>
              <select
                value={newSourceType}
                onChange={(e) => setNewSourceType(e.target.value as SourceType)}
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white focus:border-indigo-500 focus:outline-none"
              >
                {SOURCE_TYPE_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {opt.label}
                  </option>
                ))}
              </select>
            </label>

            <label className="block space-y-1.5 text-xs">
              <span className="flex items-center gap-1 font-medium text-slate-200">
                Scope
                {scopeLockedToPrivate && <Lock className="h-3 w-3 text-amber-400" />}
              </span>
              <select
                value={effectiveNewScope}
                disabled={scopeLockedToPrivate}
                onChange={(e) => setNewScope(e.target.value as SourceScope)}
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white focus:border-indigo-500 focus:outline-none disabled:opacity-60"
              >
                <option value={SourceScope.SHARED_PUBLIC}>shared_public</option>
                <option value={SourceScope.USER_PRIVATE}>user_private</option>
              </select>
            </label>

            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">Custom Name</span>
              <input
                type="text"
                value={newCustomName}
                onChange={(e) => setNewCustomName(e.target.value)}
                placeholder="e.g. Google AI Blog"
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
              />
            </label>

            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">Category</span>
              <input
                type="text"
                value={newCategory}
                onChange={(e) => setNewCategory(e.target.value)}
                placeholder="Official / Releases / Social"
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
              />
            </label>
          </div>

          <label className="block space-y-1.5 text-xs">
            <span className="font-medium text-slate-200">Handle or URL</span>
            <input
              type="text"
              value={newHandleOrUrl}
              onChange={(e) => setNewHandleOrUrl(e.target.value)}
              required
              placeholder="https://example.com/feed.xml or owner/repo or @handle"
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
            />
          </label>

          <div className="grid gap-4 rounded-lg border border-slate-800 bg-slate-950/50 p-4 sm:grid-cols-2">
            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">Bearer Token (optional)</span>
              <input
                type="password"
                autoComplete="new-password"
                value={newBearerToken}
                onChange={(e) => setNewBearerToken(e.target.value)}
                placeholder="Stored encrypted; forces user_private scope"
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
              />
            </label>

            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">API Base URL (optional, X sources)</span>
              <input
                type="url"
                value={newApiBaseUrl}
                onChange={(e) => setNewApiBaseUrl(e.target.value)}
                placeholder="https://api.x.com/2"
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
              />
            </label>
          </div>

          {scopeLockedToPrivate && (
            <p className="text-xs text-amber-300">
              Scope is locked to <span className="font-mono">user_private</span> because X
              sources and credentialed sources cannot be shared publicly.
            </p>
          )}

          <button
            type="submit"
            disabled={creating}
            className="inline-flex items-center gap-2 rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500 disabled:cursor-not-allowed disabled:opacity-50"
          >
            <Plus className="h-4 w-4" />
            {creating ? 'Adding...' : 'Add Source'}
          </button>
        </form>
      </section>

      {/* Subscribed Sources List */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-4">
        <div className="flex items-center justify-between">
          <h2 className="text-base font-semibold text-white">
            Subscribed Sources ({sources.length})
          </h2>
        </div>

        {loading ? (
          <p className="py-4 text-sm text-slate-400">Loading subscribed sources...</p>
        ) : sources.length === 0 ? (
          <p className="py-4 text-sm text-slate-500">
            No sources subscribed yet. Import the Gemini Ecosystem Preset Pack or add a custom
            source above.
          </p>
        ) : (
          <div className="space-y-3">
            {sources.map((src) => {
              const isEditing = editingId === src.id;
              const isBusy = Boolean(busyIds[src.id]);
              const testRes = testResults[src.id];
              const isPrivate = src.scope === SourceScope.USER_PRIVATE;

              return (
                <div
                  key={src.id}
                  className="rounded-lg border border-slate-800 bg-slate-950/60 p-4 space-y-3"
                >
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <div className="space-y-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="text-sm font-semibold text-white">{src.customName}</span>
                        <span className="rounded border border-slate-700 bg-slate-800 px-2 py-0.5 font-mono text-[11px] text-slate-300">
                          {src.sourceType}
                        </span>
                        <span
                          className={`rounded border px-2 py-0.5 font-mono text-[11px] ${
                            isPrivate
                              ? 'border-amber-500/30 bg-amber-500/10 text-amber-300'
                              : 'border-indigo-500/30 bg-indigo-500/10 text-indigo-300'
                          }`}
                        >
                          {src.scope}
                        </span>
                        <span className="rounded border border-slate-700 bg-slate-900 px-2 py-0.5 text-[11px] text-slate-300">
                          {src.category}
                        </span>
                        <span
                          className={`rounded-full border px-2 py-0.5 text-[11px] font-medium ${getOutcomeBadgeClass(src.lastStatus)}`}
                        >
                          {src.lastStatus ?? 'pending'}
                        </span>
                      </div>
                      <div className="text-xs text-slate-400 break-all">{src.handleOrUrl}</div>
                      {src.hasCredentials && (
                        <div className="text-[11px] text-emerald-300">
                          Credentials configured{src.configHint ? ` (${src.configHint})` : ''}
                        </div>
                      )}
                      {src.lastError && (
                        <div className="text-xs text-rose-300">
                          Error ({src.consecutiveFailures} failures): {src.lastError}
                        </div>
                      )}
                    </div>

                    <div className="flex flex-wrap items-center gap-2">
                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => void handleToggleEnabled(src)}
                        className={`rounded-lg border px-2.5 py-1.5 text-xs font-medium transition disabled:opacity-50 ${
                          src.enabled
                            ? 'border-emerald-500/40 bg-emerald-500/15 text-emerald-200 hover:bg-emerald-500/25'
                            : 'border-slate-700 bg-slate-800 text-slate-300 hover:bg-slate-700'
                        }`}
                      >
                        {src.enabled ? 'Enabled' : 'Paused'}
                      </button>

                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => void handleTestSource(src)}
                        className="inline-flex items-center gap-1 rounded-lg border border-slate-700 bg-slate-800 px-2.5 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-700 disabled:opacity-50"
                      >
                        <FlaskConical className="h-3.5 w-3.5 text-cyan-400" />
                        Test
                      </button>

                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => (isEditing ? setEditingId(null) : startEdit(src))}
                        className="inline-flex items-center gap-1 rounded-lg border border-slate-700 bg-slate-800 px-2.5 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-700 disabled:opacity-50"
                      >
                        <Edit3 className="h-3.5 w-3.5" />
                        {isEditing ? 'Cancel' : 'Edit'}
                      </button>

                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => void handleDelete(src)}
                        className="inline-flex items-center gap-1 rounded-lg border border-rose-500/40 bg-rose-500/10 px-2.5 py-1.5 text-xs font-medium text-rose-200 hover:bg-rose-500/20 disabled:opacity-50"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                        Delete
                      </button>
                    </div>
                  </div>

                  {isEditing && (
                    <div className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/70 p-4">
                      <div className="grid gap-3 sm:grid-cols-2">
                        <label className="block space-y-1 text-xs">
                          <span className="font-medium text-slate-200">Custom Name</span>
                          <input
                            type="text"
                            value={editCustomName}
                            onChange={(e) => setEditCustomName(e.target.value)}
                            className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-sm text-white"
                          />
                        </label>
                        <label className="block space-y-1 text-xs">
                          <span className="font-medium text-slate-200">Category</span>
                          <input
                            type="text"
                            value={editCategory}
                            onChange={(e) => setEditCategory(e.target.value)}
                            className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-sm text-white"
                          />
                        </label>
                      </div>

                      {isPrivate && (
                        <div className="space-y-3 border-t border-slate-800 pt-3">
                          <label className="block space-y-1 text-xs">
                            <span className="font-medium text-slate-200">Handle or URL</span>
                            <input
                              type="text"
                              value={editHandleOrUrl}
                              onChange={(e) => setEditHandleOrUrl(e.target.value)}
                              className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-sm text-white"
                            />
                          </label>
                          <div className="grid gap-3 sm:grid-cols-2">
                            <label className="block space-y-1 text-xs">
                              <span className="font-medium text-slate-200">
                                New Bearer Token (optional)
                              </span>
                              <input
                                type="password"
                                autoComplete="new-password"
                                value={editBearerToken}
                                disabled={editClearCredentials}
                                onChange={(e) => setEditBearerToken(e.target.value)}
                                placeholder="Leave blank to keep existing"
                                className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-sm text-white disabled:opacity-50"
                              />
                            </label>
                            <label className="block space-y-1 text-xs">
                              <span className="font-medium text-slate-200">
                                New API Base URL (optional)
                              </span>
                              <input
                                type="url"
                                value={editApiBaseUrl}
                                disabled={editClearCredentials}
                                onChange={(e) => setEditApiBaseUrl(e.target.value)}
                                placeholder="https://api.x.com/2"
                                className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-sm text-white disabled:opacity-50"
                              />
                            </label>
                          </div>
                          <label className="inline-flex items-center gap-2 text-xs text-slate-300">
                            <input
                              type="checkbox"
                              checked={editClearCredentials}
                              onChange={(e) => setEditClearCredentials(e.target.checked)}
                              className="h-4 w-4 rounded border-slate-600 bg-slate-900"
                            />
                            <span>Clear stored credentials</span>
                          </label>
                        </div>
                      )}

                      <div className="flex items-center gap-2">
                        <button
                          type="button"
                          disabled={savingEdit}
                          onClick={() => void handleSaveEdit(src)}
                          className="rounded-lg bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
                        >
                          {savingEdit ? 'Saving...' : 'Save Changes'}
                        </button>
                      </div>
                    </div>
                  )}

                  {testRes && (
                    <div
                      role="status"
                      className={`rounded border px-3 py-2 text-xs ${
                        testRes.outcome === FetchOutcome.FAILED
                          ? 'border-rose-500/40 bg-rose-950/30 text-rose-200'
                          : 'border-emerald-500/40 bg-emerald-950/30 text-emerald-200'
                      }`}
                    >
                      <div className="font-medium">
                        Live Test: {testRes.outcome} • {testRes.itemsFetched} items fetched
                      </div>
                      {testRes.errorMessage && (
                        <div className="mt-1 text-rose-300">{testRes.errorMessage}</div>
                      )}
                      {testRes.sampleTitles.length > 0 && (
                        <ul className="mt-1.5 list-disc space-y-0.5 pl-4 text-slate-300">
                          {testRes.sampleTitles.slice(0, 5).map((title, idx) => (
                            <li key={`${src.id}-sample-${idx}`}>{title}</li>
                          ))}
                        </ul>
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </section>

      {/* Non-Secret JSON Export & Import */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-4">
        <div>
          <h2 className="text-base font-semibold text-white">Portable JSON Export &amp; Import</h2>
          <p className="mt-0.5 text-xs text-slate-400">
            Exported bundles omit all encrypted credentials and tokens.
          </p>
        </div>

        <div className="grid gap-6 lg:grid-cols-2">
          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                disabled={exporting}
                onClick={() => void handleExport()}
                className="inline-flex items-center gap-2 rounded-lg border border-slate-700 bg-slate-800 px-3 py-2 text-xs font-medium text-slate-100 hover:bg-slate-700 disabled:opacity-50"
              >
                <Download className="h-3.5 w-3.5" />
                {exporting ? 'Exporting...' : 'Export Sources JSON'}
              </button>
              {exportedJson && (
                <button
                  type="button"
                  onClick={handleDownloadExport}
                  className="inline-flex items-center gap-2 rounded-lg border border-indigo-500/40 bg-indigo-500/15 px-3 py-2 text-xs font-medium text-indigo-200 hover:bg-indigo-500/25"
                >
                  Download .json
                </button>
              )}
            </div>
            <textarea
              rows={6}
              readOnly
              value={exportedJson}
              placeholder="Click Export Sources JSON to generate a portable non-secret bundle..."
              aria-label="Exported sources JSON"
              className="w-full rounded-lg border border-slate-700 bg-slate-950 p-3 font-mono text-xs text-slate-200 placeholder:text-slate-500"
            />
          </div>

          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <label className="inline-flex cursor-pointer items-center gap-2 rounded-lg border border-slate-700 bg-slate-800 px-3 py-2 text-xs font-medium text-slate-100 hover:bg-slate-700">
                <Upload className="h-3.5 w-3.5" />
                <span>Load JSON File</span>
                <input
                  type="file"
                  accept=".json,application/json"
                  onChange={(e) => void handleImportFile(e)}
                  className="sr-only"
                />
              </label>
              <button
                type="button"
                disabled={importingJson}
                onClick={() => void handleImportJson()}
                className="inline-flex items-center gap-2 rounded-lg bg-indigo-600 px-3 py-2 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
              >
                {importingJson ? 'Importing...' : 'Import Sources JSON'}
              </button>
            </div>
            <textarea
              rows={6}
              value={importJson}
              onChange={(e) => setImportJson(e.target.value)}
              placeholder='Paste SourceExportBundle JSON ({ "version": 1, "sources": [...] })'
              aria-label="Import sources JSON"
              className="w-full rounded-lg border border-slate-700 bg-slate-950 p-3 font-mono text-xs text-slate-200 placeholder:text-slate-500"
            />
          </div>
        </div>
      </section>
    </div>
  );
}
