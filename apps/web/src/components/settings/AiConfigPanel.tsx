import { type FormEvent, useCallback, useEffect, useState } from 'react';
import { CheckCircle2, KeyRound, PlugZap, Save, ShieldCheck, XCircle } from 'lucide-react';
import { ApiError, type ApiClient } from '../../api/client';
import { DEFAULTS } from '../../constants/routes';
import type { AiTestRequest, AiTestResult, UpdateAiConfigCmd, UserAiConfigView } from '../../types/api';

export interface AiConfigPanelProps {
  api: ApiClient;
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

export function AiConfigPanel({ api }: AiConfigPanelProps) {
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

  const loadConfig = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const view = await api.getAiConfig();
      applyView(view);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setLoading(false);
    }
  }, [api, applyView]);

  useEffect(() => {
    void loadConfig();
  }, [loadConfig]);

  const validateNumericInputs = (): { parsedTemp: number; parsedMaxTokens: number } | null => {
    const parsedTemp = Number(temperature);
    if (!Number.isFinite(parsedTemp) || parsedTemp < 0 || parsedTemp > 2) {
      setError('Temperature must be between 0.0 and 2.0.');
      return null;
    }
    const parsedMaxTokens = Number(maxTokens);
    if (!Number.isInteger(parsedMaxTokens) || parsedMaxTokens < 1) {
      setError('Max Tokens must be an integer >= 1.');
      return null;
    }
    return { parsedTemp, parsedMaxTokens };
  };

  const handleSave = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    setSaveMessage(null);

    if (!providerLabel.trim() || !baseUrl.trim() || !model.trim()) {
      setError('Provider Label, Base URL, and Model are required.');
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
      setSaveMessage('AI configuration saved.');
    } catch (err) {
      setError(formatError(err));
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
      setError(formatError(err));
    } finally {
      setTesting(false);
    }
  };

  if (loading && !config) {
    return (
      <div className="rounded-xl border border-slate-800 bg-slate-900/70 p-6 text-sm text-slate-400">
        Loading AI configuration...
      </div>
    );
  }

  return (
    <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-6 space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-white">AI Provider Configuration</h2>
          <p className="mt-1 text-xs text-slate-400">
            Configure your OpenAI-compatible summarization endpoint. Stored API keys are encrypted
            on the server and never returned in plaintext.
          </p>
        </div>
        <div className="flex items-center gap-2 rounded-full border border-slate-700 bg-slate-800/80 px-3 py-1 text-xs">
          <KeyRound className="h-3.5 w-3.5 text-indigo-400" />
          {config?.hasApiKey ? (
            <span className="text-emerald-300">
              API Key Configured{config.apiKeyHint ? ` (${config.apiKeyHint})` : ''}
            </span>
          ) : (
            <span className="text-amber-300">No API Key Stored</span>
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
            <span className="font-medium text-slate-200">Provider Label</span>
            <input
              type="text"
              value={providerLabel}
              onChange={(e) => setProviderLabel(e.target.value)}
              required
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white focus:border-indigo-500 focus:outline-none"
            />
          </label>

          <label className="block space-y-1.5 text-xs">
            <span className="font-medium text-slate-200">Model</span>
            <input
              type="text"
              value={model}
              onChange={(e) => setModel(e.target.value)}
              required
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white focus:border-indigo-500 focus:outline-none"
            />
          </label>
        </div>

        <label className="block space-y-1.5 text-xs">
          <span className="font-medium text-slate-200">Base URL</span>
          <input
            type="url"
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            required
            className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white focus:border-indigo-500 focus:outline-none"
          />
        </label>

        <div className="rounded-lg border border-slate-800 bg-slate-950/50 p-4 space-y-3">
          <label className="block space-y-1.5 text-xs">
            <span className="flex items-center gap-1.5 font-medium text-slate-200">
              <ShieldCheck className="h-3.5 w-3.5 text-emerald-400" />
              New API Key (optional, never prefilled)
            </span>
            <input
              type="password"
              autoComplete="new-password"
              value={newApiKey}
              disabled={clearApiKey}
              onChange={(e) => setNewApiKey(e.target.value)}
              placeholder={
                config?.hasApiKey
                  ? 'Leave blank to keep existing encrypted key'
                  : 'Enter provider API key'
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
            <span>Clear stored API key</span>
          </label>
        </div>

        <div className="grid gap-4 sm:grid-cols-2">
          <label className="block space-y-1.5 text-xs">
            <span className="font-medium text-slate-200">Temperature (0.0 – 2.0)</span>
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
            <span className="font-medium text-slate-200">Max Tokens (&gt;= 1)</span>
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
          <span className="font-medium text-slate-200">System Prompt</span>
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
            {saving ? 'Saving...' : 'Save AI Config'}
          </button>

          <button
            type="button"
            disabled={testing}
            onClick={() => void handleTest()}
            className="inline-flex items-center gap-2 rounded-lg border border-slate-700 bg-slate-800 px-4 py-2 text-sm font-medium text-slate-100 transition hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-50"
          >
            <PlugZap className="h-4 w-4 text-cyan-400" />
            {testing ? 'Testing...' : 'Test Connectivity'}
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
              {testResult.ok ? 'Connectivity Passed' : 'Connectivity Failed'} •{' '}
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
