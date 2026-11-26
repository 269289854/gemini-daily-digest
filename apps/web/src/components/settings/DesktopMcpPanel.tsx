import { type FormEvent, useCallback, useEffect, useRef, useState } from 'react';
import {
  Check,
  CheckCircle2,
  Copy,
  Globe,
  KeyRound,
  RefreshCw,
  ServerCog,
  XCircle,
} from 'lucide-react';
import { normalizeBaseUrl } from '../../api/client';
import { DEFAULTS, MCP_CLIENT_TARGETS, type McpClientTarget } from '../../constants/routes';
import { type TranslationKey, useI18n } from '../../i18n';
import { type McpBridgeStatus, type PlatformBridge, PlatformRuntime } from '../../platform/bridge';
import { McpAccessMode } from '../../types/enums';

export interface DesktopMcpPanelProps {
  bridge: PlatformBridge;
  serverBaseUrl: string;
  onServerBaseUrlChange: (url: string) => void;
}

export interface BuildMcpSnippetOptions {
  mcpPort: number;
  serverBaseUrl: string;
}

const MCP_MODE_OPTIONS: {
  value: McpAccessMode;
  labelKey: TranslationKey;
  descKey: TranslationKey;
}[] = [
  {
    value: McpAccessMode.DISABLED,
    labelKey: 'mcp.modeDisabledLabel',
    descKey: 'mcp.modeDisabledDesc',
  },
  {
    value: McpAccessMode.READ_ONLY,
    labelKey: 'mcp.modeReadOnlyLabel',
    descKey: 'mcp.modeReadOnlyDesc',
  },
  {
    value: McpAccessMode.READ_WRITE,
    labelKey: 'mcp.modeReadWriteLabel',
    descKey: 'mcp.modeReadWriteDesc',
  },
];

const MCP_TARGET_META: {
  target: McpClientTarget;
  labelKey: TranslationKey;
  hintKey: TranslationKey;
}[] = [
  {
    target: MCP_CLIENT_TARGETS.GEMINI_CLI,
    labelKey: 'mcp.targetGeminiCli',
    hintKey: 'mcp.targetGeminiCliHint',
  },
  {
    target: MCP_CLIENT_TARGETS.ANTIGRAVITY,
    labelKey: 'mcp.targetAntigravity',
    hintKey: 'mcp.targetAntigravityHint',
  },
  {
    target: MCP_CLIENT_TARGETS.CHATGPT,
    labelKey: 'mcp.targetChatGpt',
    hintKey: 'mcp.targetChatGptHint',
  },
  {
    target: MCP_CLIENT_TARGETS.CLAUDE_DESKTOP,
    labelKey: 'mcp.targetClaudeDesktop',
    hintKey: 'mcp.targetClaudeDesktopHint',
  },
  {
    target: MCP_CLIENT_TARGETS.CODEX,
    labelKey: 'mcp.targetCodex',
    hintKey: 'mcp.targetCodexHint',
  },
];

function sanitizePort(port: number): number {
  if (Number.isInteger(port) && port >= 1 && port <= 65535) {
    return port;
  }
  return DEFAULTS.DEFAULT_MCP_HTTP_PORT;
}

export function buildMcpClientSnippet(
  target: McpClientTarget,
  options: BuildMcpSnippetOptions,
): string {
  const safePort = sanitizePort(options.mcpPort);
  const localEndpoint = `http://${DEFAULTS.MCP_BIND_HOST}:${safePort}/mcp`;
  const trimmedServer = options.serverBaseUrl.trim();
  const resolvedServerUrl = trimmedServer || 'https://YOUR_SERVER_URL';

  switch (target) {
    case MCP_CLIENT_TARGETS.GEMINI_CLI:
      return JSON.stringify(
        {
          mcpServers: {
            'gemini-daily-digest': {
              command: 'digest-mcp',
              args: ['--transport', 'stdio'],
              env: {
                DIGEST_SERVER_URL: resolvedServerUrl,
                DIGEST_API_TOKEN: 'YOUR_MCP_TOKEN',
              },
            },
          },
        },
        null,
        2,
      );

    case MCP_CLIENT_TARGETS.ANTIGRAVITY:
      return JSON.stringify(
        {
          mcpServers: {
            'gemini-daily-digest': {
              serverUrl: localEndpoint,
              headers: {
                Authorization: 'Bearer YOUR_MCP_TOKEN',
              },
            },
          },
        },
        null,
        2,
      );

    case MCP_CLIENT_TARGETS.CHATGPT:
      return JSON.stringify(
        {
          name: 'Gemini Daily Digest',
          mcpServerUrl: localEndpoint,
          authentication: {
            type: 'bearer',
            token: 'YOUR_MCP_TOKEN',
          },
          digestServerUrl: resolvedServerUrl,
        },
        null,
        2,
      );

    case MCP_CLIENT_TARGETS.CLAUDE_DESKTOP:
      return JSON.stringify(
        {
          mcpServers: {
            'gemini-daily-digest': {
              command: 'digest-mcp',
              args: ['--transport', 'stdio'],
              env: {
                DIGEST_SERVER_URL: resolvedServerUrl,
                DIGEST_API_TOKEN: 'YOUR_MCP_TOKEN',
              },
            },
          },
        },
        null,
        2,
      );

    case MCP_CLIENT_TARGETS.CODEX:
      return JSON.stringify(
        {
          mcp_servers: {
            'gemini-daily-digest': {
              command: 'digest-mcp',
              args: ['--transport', 'stdio'],
              url: localEndpoint,
              bearer_token: 'YOUR_MCP_TOKEN',
              env: {
                DIGEST_SERVER_URL: resolvedServerUrl,
                DIGEST_API_TOKEN: 'YOUR_MCP_TOKEN',
              },
            },
          },
        },
        null,
        2,
      );
  }
}

export function DesktopMcpPanel({
  bridge,
  serverBaseUrl,
  onServerBaseUrlChange,
}: DesktopMcpPanelProps) {
  const { t } = useI18n();
  const tRef = useRef(t);
  tRef.current = t;

  const initialMcp = bridge.getMcpConfig();
  const isDesktopBridgeAvailable =
    bridge.runtime === PlatformRuntime.DESKTOP && typeof bridge.getMcpBridgeStatus === 'function';

  const [draftBaseUrl, setDraftBaseUrl] = useState<string>(serverBaseUrl);
  const [mcpMode, setMcpMode] = useState<McpAccessMode>(initialMcp.mode);
  const [mcpPortInput, setMcpPortInput] = useState<string>(String(initialMcp.port));

  const [bridgeStatus, setBridgeStatus] = useState<McpBridgeStatus | null>(null);
  const [bridgeLoading, setBridgeLoading] = useState<boolean>(false);
  const [bridgeBusy, setBridgeBusy] = useState<boolean>(false);

  const [error, setError] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [copiedTarget, setCopiedTarget] = useState<McpClientTarget | null>(null);

  useEffect(() => {
    setDraftBaseUrl(serverBaseUrl);
  }, [serverBaseUrl]);

  const refreshBridgeStatus = useCallback(async () => {
    if (!isDesktopBridgeAvailable || !bridge.getMcpBridgeStatus) {
      return;
    }
    setBridgeLoading(true);
    try {
      const nextStatus = await bridge.getMcpBridgeStatus();
      setBridgeStatus(nextStatus);
    } catch (err) {
      setError(err instanceof Error ? err.message : tRef.current('mcp.bridgeFetchError'));
    } finally {
      setBridgeLoading(false);
    }
  }, [bridge, isDesktopBridgeAvailable]);

  useEffect(() => {
    if (!isDesktopBridgeAvailable) {
      setBridgeStatus(null);
      return;
    }
    let cancelled = false;
    setBridgeLoading(true);
    void bridge
      .getMcpBridgeStatus?.()
      .then((nextStatus) => {
        if (!cancelled) {
          setBridgeStatus(nextStatus);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : tRef.current('mcp.bridgeFetchError'));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setBridgeLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [bridge, isDesktopBridgeAvailable]);

  const parsedPort = sanitizePort(Number.parseInt(mcpPortInput, 10));

  const handleSaveServerUrl = (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    setStatusMessage(null);
    try {
      const normalized = normalizeBaseUrl(draftBaseUrl);
      bridge.setServerBaseUrl(normalized);
      onServerBaseUrlChange(normalized);
      setDraftBaseUrl(normalized);
      setStatusMessage(
        normalized ? t('auth.serverUrlSet', { url: normalized }) : t('auth.serverUrlReset'),
      );
      if (isDesktopBridgeAvailable) {
        void refreshBridgeStatus();
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : t('auth.invalidServerUrl'));
    }
  };

  const handleSaveMcp = (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    setStatusMessage(null);

    const numPort = Number(mcpPortInput);
    if (!Number.isInteger(numPort) || numPort < 1 || numPort > 65535) {
      setError(t('mcp.portError'));
      return;
    }

    bridge.setMcpConfig(mcpMode, numPort);
    setStatusMessage(t('mcp.savedMsg', { mode: mcpMode, port: numPort }));
    if (isDesktopBridgeAvailable) {
      void refreshBridgeStatus();
    }
  };

  const handleCopyLocalToken = async () => {
    if (!bridge.getMcpLocalToken) {
      return;
    }
    setError(null);
    setStatusMessage(null);
    setBridgeBusy(true);
    try {
      const rawToken = await bridge.getMcpLocalToken();
      await bridge.copyToClipboard(rawToken);
      setStatusMessage(t('mcp.copiedToken'));
    } catch (err) {
      setError(err instanceof Error ? err.message : t('mcp.copyTokenError'));
    } finally {
      setBridgeBusy(false);
    }
  };

  const handleRotateLocalToken = async () => {
    if (!bridge.rotateMcpToken) {
      return;
    }
    setError(null);
    setStatusMessage(null);
    setBridgeBusy(true);
    try {
      const updatedStatus = await bridge.rotateMcpToken();
      setBridgeStatus(updatedStatus);
      setStatusMessage(t('mcp.rotatedToken'));
    } catch (err) {
      setError(err instanceof Error ? err.message : t('mcp.rotateTokenError'));
    } finally {
      setBridgeBusy(false);
    }
  };

  const handleCopySnippet = async (
    target: McpClientTarget,
    targetLabel: string,
    snippet: string,
  ) => {
    setError(null);
    try {
      await bridge.copyToClipboard(snippet);
      setCopiedTarget(target);
      setStatusMessage(t('mcp.copiedSnippet', { target: targetLabel }));
    } catch (err) {
      setError(err instanceof Error ? err.message : t('mcp.copySnippetError'));
    }
  };

  const activeModeMeta = MCP_MODE_OPTIONS.find((opt) => opt.value === mcpMode);

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

      {/* Server Base URL */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-4">
        <div className="flex items-center gap-2">
          <Globe className="h-5 w-5 text-indigo-400" />
          <div>
            <h2 className="text-base font-semibold text-white">{t('mcp.serverUrlTitle')}</h2>
            <p className="text-xs text-slate-400">
              {t('mcp.serverUrlDesc')}{' '}
              <span className="font-mono">{DEFAULTS.DEFAULT_LOCAL_SERVER_URL}</span>{' '}
              {t('mcp.serverUrlDescMid')} <span className="font-mono">{bridge.runtime}</span>.
            </p>
          </div>
        </div>

        <form onSubmit={handleSaveServerUrl} className="flex flex-wrap items-end gap-3">
          <label className="min-w-[260px] flex-1 space-y-1.5 text-xs">
            <span className="font-medium text-slate-200">{t('mcp.backendUrl')}</span>
            <input
              type="text"
              value={draftBaseUrl}
              onChange={(e) => setDraftBaseUrl(e.target.value)}
              placeholder={t('mcp.backendUrlPlaceholder', {
                localUrl: DEFAULTS.DEFAULT_LOCAL_SERVER_URL,
              })}
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500"
            />
          </label>
          <button
            type="submit"
            className="rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500"
          >
            {t('mcp.saveServerUrlBtn')}
          </button>
        </form>
      </section>

      {/* Desktop MCP Guard & Local Port */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-4">
        <div className="flex items-center gap-2">
          <ServerCog className="h-5 w-5 text-cyan-400" />
          <div>
            <h2 className="text-base font-semibold text-white">{t('mcp.localMcpTitle')}</h2>
            <p className="text-xs text-slate-400">
              {t('mcp.localMcpDescStart')}{' '}
              <span className="font-mono">{DEFAULTS.MCP_BIND_HOST}</span> {t('mcp.localMcpDescEnd')}
            </p>
          </div>
        </div>

        <form onSubmit={handleSaveMcp} className="space-y-4">
          <div className="grid gap-4 sm:grid-cols-2">
            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">{t('mcp.accessMode')}</span>
              <select
                value={mcpMode}
                onChange={(e) => setMcpMode(e.target.value as McpAccessMode)}
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              >
                {MCP_MODE_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {t(opt.labelKey)} ({opt.value})
                  </option>
                ))}
              </select>
            </label>

            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">{t('mcp.localPort')}</span>
              <input
                type="number"
                min="1"
                max="65535"
                value={mcpPortInput}
                onChange={(e) => setMcpPortInput(e.target.value)}
                required
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              />
            </label>
          </div>

          <p className="text-xs text-slate-400">
            {activeModeMeta ? t(activeModeMeta.descKey) : ''}
          </p>

          <button
            type="submit"
            className="rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500"
          >
            {t('mcp.saveMcpBtn')}
          </button>
        </form>

        {isDesktopBridgeAvailable && (
          <div className="mt-4 space-y-3 rounded-lg border border-slate-800 bg-slate-950/70 p-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex items-center gap-2">
                <span className="text-xs font-semibold uppercase tracking-wide text-slate-300">
                  {t('mcp.bridgeStatus')}
                </span>
                {bridgeStatus && (
                  <span
                    className={`rounded-full px-2 py-0.5 text-[11px] font-medium ${
                      bridgeStatus.running
                        ? 'border border-emerald-500/40 bg-emerald-500/15 text-emerald-300'
                        : 'border border-amber-500/40 bg-amber-500/15 text-amber-300'
                    }`}
                  >
                    {bridgeStatus.running ? t('mcp.running') : t('mcp.stopped')}
                  </span>
                )}
              </div>

              <div className="flex flex-wrap items-center gap-2">
                {bridge.getMcpLocalToken && (
                  <button
                    type="button"
                    disabled={bridgeBusy}
                    onClick={() => void handleCopyLocalToken()}
                    className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs font-medium text-slate-100 transition hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60"
                  >
                    <KeyRound className="h-3.5 w-3.5 text-cyan-400" />
                    {t('mcp.copyToken')}
                  </button>
                )}
                {bridge.rotateMcpToken && (
                  <button
                    type="button"
                    disabled={bridgeBusy}
                    onClick={() => void handleRotateLocalToken()}
                    className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs font-medium text-slate-100 transition hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60"
                  >
                    <RefreshCw className="h-3.5 w-3.5 text-amber-400" />
                    {t('mcp.rotateToken')}
                  </button>
                )}
                <button
                  type="button"
                  disabled={bridgeLoading || bridgeBusy}
                  onClick={() => void refreshBridgeStatus()}
                  className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-900 px-2.5 py-1.5 text-xs font-medium text-slate-300 transition hover:bg-slate-800 disabled:cursor-not-allowed disabled:opacity-60"
                >
                  <RefreshCw className={`h-3.5 w-3.5 ${bridgeLoading ? 'animate-spin' : ''}`} />
                  {t('common.refresh')}
                </button>
              </div>
            </div>

            {bridgeStatus ? (
              <div className="grid gap-2 text-xs text-slate-300 sm:grid-cols-3">
                <div>
                  <span className="text-slate-500">{t('mcp.endpoint')} </span>
                  <span className="font-mono text-slate-200">{bridgeStatus.endpointUrl}</span>
                </div>
                <div>
                  <span className="text-slate-500">{t('mcp.mode')} </span>
                  <span className="font-mono text-slate-200">{bridgeStatus.mode}</span>
                </div>
                <div>
                  <span className="text-slate-500">{t('mcp.tokenHint')} </span>
                  <span className="font-mono text-slate-200">{bridgeStatus.tokenHint}</span>
                </div>
              </div>
            ) : (
              <p className="text-xs text-slate-400">
                {bridgeLoading ? t('mcp.loadingBridge') : t('mcp.bridgeUnavailable')}
              </p>
            )}

            {bridgeStatus?.lastError && (
              <div
                role="alert"
                className="rounded-md border border-rose-500/40 bg-rose-950/40 px-3 py-2 text-xs text-rose-200"
              >
                {bridgeStatus.lastError}
              </div>
            )}
          </div>
        )}
      </section>

      {/* One-Click Client Snippets */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-4">
        <div>
          <h2 className="text-base font-semibold text-white">{t('mcp.snippetsTitle')}</h2>
          <p className="mt-0.5 text-xs text-slate-400">
            {t('mcp.snippetsDesc')} <span className="font-mono">YOUR_MCP_TOKEN</span>{' '}
            {t('mcp.snippetsDescEnd')}
          </p>
        </div>

        <div className="grid gap-4 lg:grid-cols-2">
          {MCP_TARGET_META.map(({ target, labelKey, hintKey }) => {
            const snippet = buildMcpClientSnippet(target, {
              mcpPort: parsedPort,
              serverBaseUrl: draftBaseUrl,
            });
            const isCopied = copiedTarget === target;
            const targetLabel = t(labelKey);

            return (
              <div
                key={target}
                className="flex flex-col rounded-lg border border-slate-800 bg-slate-950/70 p-4"
              >
                <div className="mb-2 flex items-center justify-between gap-2">
                  <div>
                    <h3 className="text-sm font-semibold text-white">{targetLabel}</h3>
                    <p className="text-[11px] text-slate-400">{t(hintKey)}</p>
                  </div>
                  <button
                    type="button"
                    onClick={() => void handleCopySnippet(target, targetLabel, snippet)}
                    className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-800 px-2.5 py-1.5 text-xs font-medium text-slate-100 transition hover:bg-slate-700"
                  >
                    {isCopied ? (
                      <>
                        <Check className="h-3.5 w-3.5 text-emerald-400" />
                        {t('mcp.copiedShort')}
                      </>
                    ) : (
                      <>
                        <Copy className="h-3.5 w-3.5" />
                        {t('mcp.copyJson')}
                      </>
                    )}
                  </button>
                </div>
                <pre className="flex-1 overflow-x-auto rounded border border-slate-800/80 bg-slate-900/80 p-3 font-mono text-[11px] text-slate-200">
                  <code>{snippet}</code>
                </pre>
              </div>
            );
          })}
        </div>
      </section>
    </div>
  );
}
