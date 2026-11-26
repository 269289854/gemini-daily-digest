import { type FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Check,
  CheckCircle2,
  ChevronDown,
  KeyRound,
  PlugZap,
  RefreshCw,
  Save,
  ShieldCheck,
  XCircle,
} from 'lucide-react';
import { ApiError, type ApiClient } from '../../api/client';
import { DEFAULTS } from '../../constants/routes';
import { type TranslateFn, useI18n } from '../../i18n';
import type {
  AiModelOption,
  AiModelsRequest,
  AiTestRequest,
  AiTestResult,
  UpdateAiConfigCmd,
  UserAiConfigView,
} from '../../types/api';

export interface AiConfigPanelProps {
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

export function AiConfigPanel({ api }: AiConfigPanelProps) {
  const { t } = useI18n();
  const tRef = useRef(t);
  tRef.current = t;

  const [config, setConfig] = useState<UserAiConfigView | null>(null);
  const [providerLabel, setProviderLabel] = useState<string>(DEFAULTS.AI_PROVIDER_LABEL);
  const [baseUrl, setBaseUrl] = useState<string>(DEFAULTS.AI_BASE_URL);
  const [model, setModel] = useState<string>(DEFAULTS.AI_MODEL);
  const [systemPrompt, setSystemPrompt] = useState<string>(DEFAULTS.LLM_SYSTEM_PROMPT);
  const [temperature, setTemperature] = useState<string>(String(DEFAULTS.LLM_TEMPERATURE));
  const [maxTokens, setMaxTokens] = useState<string>(String(DEFAULTS.LLM_MAX_TOKENS));
  const [newApiKey, setNewApiKey] = useState<string>('');
  const [clearApiKey, setClearApiKey] = useState<boolean>(false);

  const [loading, setLoading] = useState<boolean>(true);
  const [saving, setSaving] = useState<boolean>(false);
  const [testing, setTesting] = useState<boolean>(false);
  const [fetchingModels, setFetchingModels] = useState<boolean>(false);
  const [availableModels, setAvailableModels] = useState<AiModelOption[]>([]);
  const [modelsMeta, setModelsMeta] = useState<{ count: number; ms: number } | null>(null);
  const [modelsDropdownOpen, setModelsDropdownOpen] = useState<boolean>(false);
  const modelPickerRef = useRef<HTMLDivElement | null>(null);

  const [error, setError] = useState<string | null>(null);
  const [saveMessage, setSaveMessage] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<AiTestResult | null>(null);

  const applyView = useCallback((view: UserAiConfigView) => {
    setConfig(view);
    setProviderLabel(view.providerLabel);
    setBaseUrl(view.baseUrl);
    setModel(view.model);
    setSystemPrompt(view.systemPrompt);
    setTemperature(String(view.temperature));
    setMaxTokens(String(view.maxTokens));
    setNewApiKey('');
    setClearApiKey(false);
  }, []);

  const discoverModels = useCallback(
    async (opts: {
      targetBaseUrl: string;
      draftApiKey: string;
      clearKey: boolean;
      silent: boolean;
      openOnSuccess: boolean;
    }) => {
      if (typeof api.listAiModels !== 'function') {
        return;
      }
      const normalizedUrl = opts.targetBaseUrl.trim();
      if (!normalizedUrl) {
        if (!opts.silent) {
          setError(tRef.current('ai.baseUrlRequiredForModels'));
        }
        return;
      }
      if (!opts.silent) {
        setError(null);
      }
      setFetchingModels(true);
      try {
        const req: AiModelsRequest = {
          baseUrl: normalizedUrl,
          clearApiKey: opts.clearKey,
        };
        const trimmedKey = opts.draftApiKey.trim();
        if (!opts.clearKey && trimmedKey.length > 0) {
          req.apiKey = trimmedKey;
        }
        const res = await api.listAiModels(req);
        setAvailableModels(res.models);
        setModelsMeta({ count: res.models.length, ms: res.latencyMs });
        if (opts.openOnSuccess && res.models.length > 0) {
          setModelsDropdownOpen(true);
        }
      } catch (err) {
        if (!opts.silent) {
          setError(formatError(err, tRef.current));
        }
      } finally {
        setFetchingModels(false);
      }
    },
    [api],
  );

  const loadConfig = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const view = await api.getAiConfig();
      applyView(view);
      if (view.hasApiKey && typeof api.listAiModels === 'function') {
        void discoverModels({
          targetBaseUrl: view.baseUrl,
          draftApiKey: '',
          clearKey: false,
          silent: true,
          openOnSuccess: false,
        });
      }
    } catch (err) {
      setError(formatError(err, tRef.current));
    } finally {
      setLoading(false);
    }
  }, [api, applyView, discoverModels]);

  useEffect(() => {
    void loadConfig();
  }, [loadConfig]);

  useEffect(() => {
    if (!modelsDropdownOpen) {
      return undefined;
    }
    const handlePointerDown = (event: MouseEvent) => {
      if (modelPickerRef.current && !modelPickerRef.current.contains(event.target as Node)) {
        setModelsDropdownOpen(false);
      }
    };
    document.addEventListener('mousedown', handlePointerDown);
    return () => {
      document.removeEventListener('mousedown', handlePointerDown);
    };
  }, [modelsDropdownOpen]);

  const validateNumericInputs = (): { parsedTemp: number; parsedMaxTokens: number } | null => {
    const parsedTemp = Number(temperature);
    if (!Number.isFinite(parsedTemp) || parsedTemp < 0 || parsedTemp > 2) {
      setError(t('ai.tempRangeError'));
      return null;
    }
    const parsedMaxTokens = Number(maxTokens);
    if (!Number.isInteger(parsedMaxTokens) || parsedMaxTokens < 1) {
      setError(t('ai.maxTokensError'));
      return null;
    }
    return { parsedTemp, parsedMaxTokens };
  };

  const handleFetchModels = () => {
    void discoverModels({
      targetBaseUrl: baseUrl,
      draftApiKey: newApiKey,
      clearKey: clearApiKey,
      silent: false,
      openOnSuccess: true,
    });
  };

  const filteredModels = useMemo(() => {
    const query = model.trim().toLowerCase();
    if (!query || availableModels.some((item) => item.id.toLowerCase() === query)) {
      return availableModels;
    }
    return availableModels.filter(
      (item) =>
        item.id.toLowerCase().includes(query) ||
        (item.ownedBy ? item.ownedBy.toLowerCase().includes(query) : false),
    );
  }, [availableModels, model]);

  const trimmedModel = model.trim();
  const isCustomModelOutsideList =
    availableModels.length > 0 &&
    trimmedModel.length > 0 &&
    !availableModels.some((item) => item.id === trimmedModel);

  const handleSave = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    setSaveMessage(null);
    setModelsDropdownOpen(false);

    if (!providerLabel.trim() || !baseUrl.trim() || !model.trim()) {
      setError(t('ai.requiredFieldsError'));
      return;
    }
    const nums = validateNumericInputs();
    if (!nums) {
      return;
    }

    const trimmedKey = newApiKey.trim();
    const payload: UpdateAiConfigCmd = {
      providerLabel: providerLabel.trim(),
      baseUrl: baseUrl.trim(),
      model: model.trim(),
      systemPrompt: systemPrompt.trim() || undefined,
      temperature: nums.parsedTemp,
      maxTokens: nums.parsedMaxTokens,
      clearApiKey,
    };
    if (!clearApiKey && trimmedKey.length > 0) {
      payload.apiKey = trimmedKey;
    }

    setSaving(true);
    try {
      const updated = await api.updateAiConfig(payload);
      applyView(updated);
      setSaveMessage(t('ai.saved'));
    } catch (err) {
      setError(formatError(err, t));
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async () => {
    setError(null);
    setTestResult(null);
    const nums = validateNumericInputs();
    if (!nums) {
      return;
    }

    const req: AiTestRequest = {
      providerLabel: providerLabel.trim() || undefined,
      baseUrl: baseUrl.trim() || undefined,
      model: model.trim() || undefined,
      systemPrompt: systemPrompt.trim() || undefined,
      temperature: nums.parsedTemp,
      maxTokens: nums.parsedMaxTokens,
    };
    const trimmedKey = newApiKey.trim();
    if (!clearApiKey && trimmedKey.length > 0) {
      req.apiKey = trimmedKey;
    }

    setTesting(true);
    try {
      const result = await api.testAiConfig(req);
      setTestResult(result);
    } catch (err) {
      setError(formatError(err, t));
    } finally {
      setTesting(false);
    }
  };

  if (loading && !config) {
    return (
      <div className="rounded-xl border border-slate-800 bg-slate-900/70 p-6 text-sm text-slate-400">
        {t('ai.loading')}
      </div>
    );
  }

  return (
    <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-6 space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-white">{t('ai.title')}</h2>
          <p className="mt-1 text-xs text-slate-400">{t('ai.subtitle')}</p>
        </div>
        <div className="flex items-center gap-2 rounded-full border border-slate-700 bg-slate-800/80 px-3 py-1 text-xs">
          <KeyRound className="h-3.5 w-3.5 text-indigo-400" />
          {config?.hasApiKey ? (
            <span className="text-emerald-300">
              {t('ai.keyConfigured')}
              {config.apiKeyHint ? ` (${config.apiKeyHint})` : ''}
            </span>
          ) : (
            <span className="text-amber-300">{t('ai.noKeyStored')}</span>
          )}
        </div>
      </div>

      {error && (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-lg border border-rose-500/40 bg-rose-950/50 px-4 py-3 text-sm text-rose-200"
        >
          <XCircle className="mt-0.5 h-4 w-4 shrink-0 text-rose-400" />
          <span>{error}</span>
        </div>
      )}

      {saveMessage && (
        <div
          role="status"
          className="flex items-start gap-2 rounded-lg border border-emerald-500/40 bg-emerald-950/40 px-4 py-3 text-sm text-emerald-200"
        >
          <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-emerald-400" />
          <span>{saveMessage}</span>
        </div>
      )}

      <form onSubmit={(e) => void handleSave(e)} className="space-y-4">
        <div className="grid gap-4 sm:grid-cols-2">
          <label className="block space-y-1.5 text-xs">
            <span className="font-medium text-slate-200">{t('ai.providerLabel')}</span>
            <input
              type="text"
              value={providerLabel}
              onChange={(e) => setProviderLabel(e.target.value)}
              required
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white focus:border-indigo-500 focus:outline-none"
            />
          </label>

          <label className="block space-y-1.5 text-xs">
            <span className="font-medium text-slate-200">{t('ai.baseUrl')}</span>
            <input
              type="url"
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              required
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white focus:border-indigo-500 focus:outline-none"
            />
          </label>
        </div>

        <div className="rounded-lg border border-slate-800 bg-slate-950/50 p-4 space-y-3">
          <label className="block space-y-1.5 text-xs">
            <span className="flex items-center gap-1.5 font-medium text-slate-200">
              <ShieldCheck className="h-3.5 w-3.5 text-emerald-400" />
              {t('ai.newApiKey')}
            </span>
            <input
              type="password"
              autoComplete="new-password"
              value={newApiKey}
              disabled={clearApiKey}
              onChange={(e) => setNewApiKey(e.target.value)}
              placeholder={
                config?.hasApiKey
                  ? t('ai.newApiKeyKeepPlaceholder')
                  : t('ai.newApiKeyEnterPlaceholder')
              }
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none disabled:opacity-50"
            />
          </label>

          <label className="inline-flex items-center gap-2 text-xs text-slate-300">
            <input
              type="checkbox"
              checked={clearApiKey}
              onChange={(e) => setClearApiKey(e.target.checked)}
              className="h-4 w-4 rounded border-slate-600 bg-slate-900 text-indigo-500 focus:ring-indigo-500"
            />
            <span>{t('ai.clearApiKey')}</span>
          </label>
        </div>

        <div ref={modelPickerRef} className="relative space-y-1.5">
          <label htmlFor="ai-model-input" className="block text-xs font-medium text-slate-200">
            {t('ai.model')}
          </label>
          <div className="flex flex-col gap-2 sm:flex-row sm:items-stretch">
            <div className="relative flex-1">
              <input
                id="ai-model-input"
                type="text"
                role="combobox"
                aria-expanded={modelsDropdownOpen}
                aria-controls="ai-model-listbox"
                aria-autocomplete="list"
                value={model}
                onChange={(e) => {
                  setModel(e.target.value);
                  if (availableModels.length > 0) {
                    setModelsDropdownOpen(true);
                  }
                }}
                onFocus={() => {
                  if (availableModels.length > 0) {
                    setModelsDropdownOpen(true);
                  }
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Escape') {
                    setModelsDropdownOpen(false);
                  }
                }}
                placeholder={t('ai.modelPlaceholder')}
                required
                className={`w-full rounded-lg border border-slate-700 bg-slate-950 py-2 pl-3 text-sm text-white placeholder:text-slate-500 focus:border-indigo-500 focus:outline-none ${
                  availableModels.length > 0 ? 'pr-9' : 'pr-3'
                }`}
              />
              {availableModels.length > 0 && (
                <button
                  type="button"
                  aria-label={t('ai.toggleModelsDropdownAria')}
                  onClick={() => setModelsDropdownOpen((prev) => !prev)}
                  className="absolute inset-y-0 right-0 flex items-center px-2.5 text-slate-400 hover:text-white"
                >
                  <ChevronDown
                    className={`h-4 w-4 transition-transform ${
                      modelsDropdownOpen ? 'rotate-180' : ''
                    }`}
                  />
                </button>
              )}
            </div>

            <button
              type="button"
              disabled={fetchingModels}
              onClick={handleFetchModels}
              className="inline-flex shrink-0 items-center justify-center gap-1.5 rounded-lg border border-slate-700 bg-slate-800 px-3.5 py-2 text-xs font-medium text-slate-100 transition hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-50"
            >
              <RefreshCw className={`h-3.5 w-3.5 text-indigo-400 ${fetchingModels ? 'animate-spin' : ''}`} />
              {fetchingModels
                ? t('ai.fetchingModelsBtn')
                : availableModels.length > 0
                  ? t('ai.refreshModelsBtn')
                  : t('ai.fetchModelsBtn')}
            </button>
          </div>

          {modelsDropdownOpen && availableModels.length > 0 && (
            <div
              id="ai-model-listbox"
              role="listbox"
              aria-label={t('ai.modelsDropdownAria')}
              className="absolute z-20 mt-1 max-h-60 w-full overflow-y-auto rounded-lg border border-slate-700 bg-slate-950 p-1 shadow-xl"
            >
              {filteredModels.length > 0 ? (
                filteredModels.map((opt) => {
                  const selected = opt.id === model;
                  return (
                    <button
                      key={opt.id}
                      type="button"
                      role="option"
                      aria-selected={selected}
                      onClick={() => {
                        setModel(opt.id);
                        setModelsDropdownOpen(false);
                      }}
                      className={`flex w-full items-center justify-between gap-2 rounded-md px-2.5 py-1.5 text-left text-xs transition ${
                        selected
                          ? 'bg-indigo-600/25 text-indigo-200'
                          : 'text-slate-200 hover:bg-slate-800'
                      }`}
                    >
                      <span className="flex items-center gap-2 truncate font-mono">
                        <Check
                          className={`h-3.5 w-3.5 shrink-0 ${
                            selected ? 'text-indigo-400 opacity-100' : 'opacity-0'
                          }`}
                        />
                        <span className="truncate">{opt.id}</span>
                      </span>
                      {opt.ownedBy && (
                        <span className="shrink-0 rounded bg-slate-800 px-1.5 py-0.5 text-[10px] text-slate-400">
                          {opt.ownedBy}
                        </span>
                      )}
                    </button>
                  );
                })
              ) : (
                <div className="px-2.5 py-2 text-xs text-slate-400">
                  {t('ai.noMatchingModels')}
                </div>
              )}
            </div>
          )}

          {modelsMeta && (
            <p className="text-[11px] text-slate-400">
              {modelsMeta.count > 0
                ? t('ai.modelsLoadedStatus', { count: modelsMeta.count, ms: modelsMeta.ms })
                : t('ai.modelsEmptyStatus')}
            </p>
          )}
          {isCustomModelOutsideList && (
            <p className="text-[11px] text-amber-300">
              {t('ai.customModelHint', { model: trimmedModel })}
            </p>
          )}
        </div>

        <div className="grid gap-4 sm:grid-cols-2">
          <label className="block space-y-1.5 text-xs">
            <span className="font-medium text-slate-200">{t('ai.temperature')}</span>
            <input
              type="number"
              step="0.1"
              min="0"
              max="2"
              value={temperature}
              onChange={(e) => setTemperature(e.target.value)}
              required
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white focus:border-indigo-500 focus:outline-none"
            />
          </label>

          <label className="block space-y-1.5 text-xs">
            <span className="font-medium text-slate-200">{t('ai.maxTokens')}</span>
            <input
              type="number"
              step="1"
              min="1"
              value={maxTokens}
              onChange={(e) => setMaxTokens(e.target.value)}
              required
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white focus:border-indigo-500 focus:outline-none"
            />
          </label>
        </div>

        <label className="block space-y-1.5 text-xs">
          <span className="font-medium text-slate-200">{t('ai.systemPrompt')}</span>
          <textarea
            rows={4}
            value={systemPrompt}
            onChange={(e) => setSystemPrompt(e.target.value)}
            className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white focus:border-indigo-500 focus:outline-none"
          />
        </label>

        <div className="flex flex-wrap items-center gap-3 pt-2">
          <button
            type="submit"
            disabled={saving}
            className="inline-flex items-center gap-2 rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500 disabled:cursor-not-allowed disabled:opacity-50"
          >
            <Save className="h-4 w-4" />
            {saving ? t('common.saving') : t('ai.saveBtn')}
          </button>

          <button
            type="button"
            disabled={testing}
            onClick={() => void handleTest()}
            className="inline-flex items-center gap-2 rounded-lg border border-slate-700 bg-slate-800 px-4 py-2 text-sm font-medium text-slate-100 transition hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-50"
          >
            <PlugZap className="h-4 w-4 text-cyan-400" />
            {testing ? t('ai.testingBtn') : t('ai.testBtn')}
          </button>
        </div>
      </form>

      {testResult && (
        <div
          role="status"
          className={`rounded-lg border px-4 py-3 text-xs ${
            testResult.ok
              ? 'border-emerald-500/40 bg-emerald-950/40 text-emerald-200'
              : 'border-rose-500/40 bg-rose-950/40 text-rose-200'
          }`}
        >
          <div className="flex flex-wrap items-center justify-between gap-2 font-semibold">
            <span>
              {testResult.ok ? t('ai.testPassed') : t('ai.testFailed')} •{' '}
              {testResult.providerLabel} ({testResult.model})
            </span>
            <span>{testResult.latencyMs} ms</span>
          </div>
          <p className="mt-1 opacity-90">{testResult.message}</p>
        </div>
      )}
    </section>
  );
}
