import { type FormEvent, useCallback, useEffect, useRef, useState } from 'react';
import {
  CheckCircle2,
  FlaskConical,
  KeyRound,
  RefreshCw,
  Save,
  ShieldCheck,
  XCircle,
} from 'lucide-react';
import { ApiError, type ApiClient } from '../../api/client';
import { type TranslateFn, useI18n } from '../../i18n';
import type {
  PlatformSettingsView,
  PlatformXTestResult,
  UpdatePlatformSettingsCmd,
} from '../../types/api';

export interface PlatformAdminPanelProps {
  api: ApiClient;
}

function formatError(err: unknown, t: TranslateFn): string {
  if (err instanceof ApiError) {
    return err.requestId
      ? `${err.message} (${t('common.requestId', { id: err.requestId })})`
      : err.message;
  }
  if (err instanceof Error) {
    return err.message;
  }
  return t('common.unexpectedError');
}

export function PlatformAdminPanel({ api }: PlatformAdminPanelProps) {
  const { t } = useI18n();
  const tRef = useRef(t);
  tRef.current = t;

  const [view, setView] = useState<PlatformSettingsView | null>(null);
  const [xBridgeBaseUrl, setXBridgeBaseUrl] = useState<string>('');
  const [xBridgeReloadUrl, setXBridgeReloadUrl] = useState<string>('');
  const [newReloadSecret, setNewReloadSecret] = useState<string>('');
  const [clearReloadSecret, setClearReloadSecret] = useState<boolean>(false);
  const [newAuthToken, setNewAuthToken] = useState<string>('');
  const [clearAuthToken, setClearAuthToken] = useState<boolean>(false);
  const [newBearerToken, setNewBearerToken] = useState<string>('');
  const [clearBearerToken, setClearBearerToken] = useState<boolean>(false);
  const [testHandle, setTestHandle] = useState<string>('GoogleDeepMind');

  const [loading, setLoading] = useState<boolean>(true);
  const [saving, setSaving] = useState<boolean>(false);
  const [testing, setTesting] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);
  const [saveMessage, setSaveMessage] = useState<string | null>(null);
  const [saveWarning, setSaveWarning] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<PlatformXTestResult | null>(null);

  const applyView = useCallback((next: PlatformSettingsView) => {
    setView(next);
    setXBridgeBaseUrl(next.xBridgeBaseUrl ?? '');
    setXBridgeReloadUrl(next.xBridgeReloadUrl ?? '');
    setNewReloadSecret('');
    setClearReloadSecret(false);
    setNewAuthToken('');
    setClearAuthToken(false);
    setNewBearerToken('');
    setClearBearerToken(false);
  }, []);

  const loadSettings = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const current = await api.getAdminPlatformSettings();
      applyView(current);
    } catch (err) {
      setError(formatError(err, tRef.current));
    } finally {
      setLoading(false);
    }
  }, [api, applyView]);

  useEffect(() => {
    void loadSettings();
  }, [loadSettings]);

  const handleSave = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    setSaveMessage(null);
    setSaveWarning(null);

    const cmd: UpdatePlatformSettingsCmd = {
      xBridgeBaseUrl: xBridgeBaseUrl.trim(),
      xBridgeReloadUrl: xBridgeReloadUrl.trim(),
      clearXBridgeReloadSecret: clearReloadSecret,
      clearXAuthToken: clearAuthToken,
      clearXBearerToken: clearBearerToken,
    };
    if (!clearReloadSecret && newReloadSecret.trim()) {
      cmd.xBridgeReloadSecret = newReloadSecret.trim();
    }
    if (!clearAuthToken && newAuthToken.trim()) {
      cmd.xAuthToken = newAuthToken.trim();
    }
    if (!clearBearerToken && newBearerToken.trim()) {
      cmd.xBearerToken = newBearerToken.trim();
    }

    setSaving(true);
    try {
      const res = await api.updateAdminPlatformSettings(cmd);
      applyView(res.settings);
      if (res.bridgeReloaded) {
        setSaveMessage(
          t('platformAdmin.savedReloaded', {
            message: res.bridgeMessage ?? 'OK',
          }),
        );
      } else if (res.bridgeMessage && (cmd.xAuthToken || cmd.clearXAuthToken)) {
        setSaveWarning(t('platformAdmin.reloadWarning', { message: res.bridgeMessage }));
      } else {
        setSaveMessage(t('platformAdmin.savedSimple'));
      }
    } catch (err) {
      setError(formatError(err, t));
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async () => {
    setError(null);
    setTestResult(null);
    setTesting(true);
    try {
      const res = await api.testAdminPlatformX({
        handle: testHandle.trim() || undefined,
      });
      setTestResult(res);
    } catch (err) {
      setError(formatError(err, t));
    } finally {
      setTesting(false);
    }
  };

  if (loading) {
    return (
      <div className="rounded-xl border border-slate-800 bg-slate-900/70 p-6 text-sm text-slate-400">
        {t('platformAdmin.loading')}
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-6">
        <div className="flex flex-wrap items-start justify-between gap-3 border-b border-slate-800 pb-4">
          <div>
            <h2 className="text-lg font-semibold text-white">{t('platformAdmin.title')}</h2>
            <p className="mt-1 text-xs text-slate-400">{t('platformAdmin.subtitle')}</p>
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs">
            {view?.xBridgeConfigured && (
              <span className="inline-flex items-center gap-1.5 rounded-full border border-sky-500/30 bg-sky-500/10 px-3 py-1 font-medium text-sky-300">
                <RefreshCw className="h-3.5 w-3.5" />
                {t('platformAdmin.statusBridgeReady')}
              </span>
            )}
            {view?.hasXAuthToken ? (
              <span className="inline-flex items-center gap-1.5 rounded-full border border-emerald-500/30 bg-emerald-500/15 px-3 py-1 font-medium text-emerald-300">
                <ShieldCheck className="h-3.5 w-3.5" />
                {t('platformAdmin.statusTokenSaved')}
                {view.xAuthTokenHint ? ` (${view.xAuthTokenHint})` : ''}
              </span>
            ) : (
              <span className="inline-flex items-center gap-1.5 rounded-full border border-amber-500/30 bg-amber-500/10 px-3 py-1 font-medium text-amber-300">
                <KeyRound className="h-3.5 w-3.5" />
                {t('platformAdmin.statusNoAuthToken')}
              </span>
            )}
            {view?.hasXBearerToken && (
              <span className="inline-flex items-center gap-1.5 rounded-full border border-indigo-500/30 bg-indigo-500/15 px-3 py-1 font-medium text-indigo-300">
                <KeyRound className="h-3.5 w-3.5" />
                {t('platformAdmin.statusBearerSaved')}
                {view.xBearerTokenHint ? ` (${view.xBearerTokenHint})` : ''}
              </span>
            )}
          </div>
        </div>

        {error && (
          <div
            role="alert"
            className="mt-4 flex items-start gap-2 rounded-lg border border-rose-500/40 bg-rose-950/50 px-4 py-3 text-sm text-rose-200"
          >
            <XCircle className="mt-0.5 h-4 w-4 shrink-0 text-rose-400" />
            <span>{error}</span>
          </div>
        )}

        {saveMessage && (
          <div
            role="status"
            className="mt-4 flex items-start gap-2 rounded-lg border border-emerald-500/40 bg-emerald-950/40 px-4 py-3 text-sm text-emerald-200"
          >
            <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-emerald-400" />
            <span>{saveMessage}</span>
          </div>
        )}

        {saveWarning && (
          <div
            role="status"
            className="mt-4 flex items-start gap-2 rounded-lg border border-amber-500/40 bg-amber-950/40 px-4 py-3 text-sm text-amber-200"
          >
            <RefreshCw className="mt-0.5 h-4 w-4 shrink-0 text-amber-400" />
            <span>{saveWarning}</span>
          </div>
        )}

        <form onSubmit={(e) => void handleSave(e)} className="mt-5 space-y-5">
          <div className="space-y-1.5">
            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">
                {t('platformAdmin.authTokenLabel')}
              </span>
              <input
                type="password"
                autoComplete="new-password"
                aria-label={t('platformAdmin.authTokenLabel')}
                value={newAuthToken}
                disabled={clearAuthToken}
                onChange={(e) => setNewAuthToken(e.target.value)}
                placeholder={t('platformAdmin.authTokenPlaceholder')}
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none disabled:opacity-50"
              />
            </label>
            <p className="text-[11px] text-slate-400">{t('platformAdmin.authTokenHint')}</p>
            <label className="inline-flex items-center gap-2 pt-1 text-xs text-slate-300">
              <input
                type="checkbox"
                checked={clearAuthToken}
                onChange={(e) => setClearAuthToken(e.target.checked)}
                className="h-4 w-4 rounded border-slate-600 bg-slate-900"
              />
              <span>{t('platformAdmin.clearAuthToken')}</span>
            </label>
          </div>

          <div className="space-y-1.5">
            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">
                {t('platformAdmin.bridgeUrlLabel')}
              </span>
              <input
                type="url"
                aria-label={t('platformAdmin.bridgeUrlLabel')}
                value={xBridgeBaseUrl}
                onChange={(e) => setXBridgeBaseUrl(e.target.value)}
                placeholder={t('platformAdmin.bridgeUrlPlaceholder')}
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
              />
            </label>
            <p className="text-[11px] text-slate-400">{t('platformAdmin.bridgeUrlHint')}</p>
          </div>

          <details className="rounded-lg border border-slate-800 bg-slate-950/50 p-4">
            <summary className="cursor-pointer text-xs font-medium text-slate-300 hover:text-white">
              {t('platformAdmin.advancedSummary')}
            </summary>
            <div className="mt-4 space-y-4">
              <label className="block space-y-1.5 text-xs">
                <span className="font-medium text-slate-200">
                  {t('platformAdmin.reloadUrlLabel')}
                </span>
                <input
                  type="url"
                  aria-label={t('platformAdmin.reloadUrlLabel')}
                  value={xBridgeReloadUrl}
                  onChange={(e) => setXBridgeReloadUrl(e.target.value)}
                  placeholder={t('platformAdmin.reloadUrlPlaceholder')}
                  className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
                />
              </label>

              <div className="space-y-1.5">
                <label className="block space-y-1.5 text-xs">
                  <span className="font-medium text-slate-200">
                    {t('platformAdmin.reloadSecretLabel')}
                  </span>
                  <input
                    type="password"
                    autoComplete="new-password"
                    aria-label={t('platformAdmin.reloadSecretLabel')}
                    value={newReloadSecret}
                    disabled={clearReloadSecret}
                    onChange={(e) => setNewReloadSecret(e.target.value)}
                    placeholder={t('platformAdmin.reloadSecretPlaceholder')}
                    className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none disabled:opacity-50"
                  />
                </label>
                <label className="inline-flex items-center gap-2 text-xs text-slate-300">
                  <input
                    type="checkbox"
                    checked={clearReloadSecret}
                    onChange={(e) => setClearReloadSecret(e.target.checked)}
                    className="h-4 w-4 rounded border-slate-600 bg-slate-900"
                  />
                  <span>{t('platformAdmin.clearReloadSecret')}</span>
                </label>
              </div>

              <div className="space-y-1.5 border-t border-slate-800 pt-3">
                <label className="block space-y-1.5 text-xs">
                  <span className="font-medium text-slate-200">
                    {t('platformAdmin.bearerTokenLabel')}
                  </span>
                  <input
                    type="password"
                    autoComplete="new-password"
                    aria-label={t('platformAdmin.bearerTokenLabel')}
                    value={newBearerToken}
                    disabled={clearBearerToken}
                    onChange={(e) => setNewBearerToken(e.target.value)}
                    placeholder={t('platformAdmin.bearerTokenPlaceholder')}
                    className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none disabled:opacity-50"
                  />
                </label>
                <label className="inline-flex items-center gap-2 text-xs text-slate-300">
                  <input
                    type="checkbox"
                    checked={clearBearerToken}
                    onChange={(e) => setClearBearerToken(e.target.checked)}
                    className="h-4 w-4 rounded border-slate-600 bg-slate-900"
                  />
                  <span>{t('platformAdmin.clearBearerToken')}</span>
                </label>
              </div>
            </div>
          </details>

          <div className="flex flex-wrap items-end gap-3 border-t border-slate-800 pt-4">
            <button
              type="submit"
              disabled={saving}
              className="inline-flex items-center gap-2 rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500 disabled:cursor-not-allowed disabled:opacity-50"
            >
              <Save className="h-4 w-4" />
              {saving ? t('platformAdmin.savingBtn') : t('platformAdmin.saveBtn')}
            </button>

            <div className="flex items-end gap-2">
              <label className="block space-y-1 text-xs">
                <span className="font-medium text-slate-300">
                  {t('platformAdmin.testHandleLabel')}
                </span>
                <input
                  type="text"
                  aria-label={t('platformAdmin.testHandleLabel')}
                  value={testHandle}
                  onChange={(e) => setTestHandle(e.target.value)}
                  placeholder={t('platformAdmin.testHandlePlaceholder')}
                  className="w-44 rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none"
                />
              </label>
              <button
                type="button"
                disabled={testing}
                onClick={() => void handleTest()}
                className="inline-flex items-center gap-2 rounded-lg border border-slate-700 bg-slate-800 px-4 py-2 text-sm font-medium text-slate-100 transition hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-50"
              >
                <FlaskConical className="h-4 w-4 text-cyan-400" />
                {testing ? t('platformAdmin.testingBtn') : t('platformAdmin.testBtn')}
              </button>
            </div>
          </div>
        </form>

        {testResult && (
          <div
            role="status"
            className={`mt-4 rounded-lg border p-4 text-xs ${
              testResult.ok
                ? 'border-emerald-500/40 bg-emerald-950/30 text-emerald-200'
                : 'border-rose-500/40 bg-rose-950/30 text-rose-200'
            }`}
          >
            <div className="font-semibold">
              {testResult.ok
                ? t('platformAdmin.testPassed', {
                    count: testResult.itemsFetched,
                    ms: testResult.latencyMs,
                    mode: testResult.mode,
                  })
                : t('platformAdmin.testFailed', {
                    ms: testResult.latencyMs,
                    mode: testResult.mode,
                  })}
            </div>
            <p className="mt-1 opacity-90">{testResult.message}</p>
            {testResult.sampleTitles.length > 0 && (
              <ul className="mt-2 list-disc space-y-1 pl-4 text-slate-200">
                {testResult.sampleTitles.slice(0, 5).map((title, idx) => (
                  <li key={`platform-x-sample-${idx}`}>{title}</li>
                ))}
              </ul>
            )}
          </div>
        )}
      </section>
    </div>
  );
}
