import { type FormEvent, useCallback, useEffect, useState } from 'react';
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

const MCP_MODE_OPTIONS: { value: McpAccessMode; label: string; description: string }[] = [
  {
    value: McpAccessMode.DISABLED,
    label: 'Disabled',
    description: 'Block all local MCP tool invocations.',
  },
  {
    value: McpAccessMode.READ_ONLY,
    label: 'Read-Only',
    description: 'Allow querying digests, sources, bookmarks, and stats only.',
  },
  {
    value: McpAccessMode.READ_WRITE,
    label: 'Read / Write',
    description: 'Allow read tools plus triggering runs, adding sources, and updating schedules.',
  },
];

const MCP_TARGET_META: { target: McpClientTarget; label: string; hint: string }[] = [
  {
    target: MCP_CLIENT_TARGETS.GEMINI_CLI,
    label: 'Gemini CLI',
    hint: '~/.gemini/settings.json (mcpServers)',
  },
  {
    target: MCP_CLIENT_TARGETS.ANTIGRAVITY,
    label: 'Antigravity',
    hint: 'Antigravity MCP configuration (Streamable HTTP)',
  },
  {
    target: MCP_CLIENT_TARGETS.CHATGPT,
    label: 'ChatGPT',
    hint: 'Custom MCP connector URL + Bearer token',
  },
  {
    target: MCP_CLIENT_TARGETS.CLAUDE_DESKTOP,
    label: 'Claude Desktop',
    hint: 'claude_desktop_config.json (mcpServers)',
  },
  {
    target: MCP_CLIENT_TARGETS.CODEX,
    label: 'Codex',
    hint: 'Codex mcp_servers JSON configuration',
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
      setError(err instanceof Error ? err.message : 'Failed to fetch Desktop MCP bridge status');
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
          setError(err instanceof Error ? err.message : 'Failed to fetch Desktop MCP bridge status');
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
        normalized
          ? `Server Base URL saved (${normalized}).`
          : 'Server Base URL reset to same-origin relative mode.',
      );
      if (isDesktopBridgeAvailable) {
        void refreshBridgeStatus();
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Invalid server base URL');
    }
  };

  const handleSaveMcp = (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    setStatusMessage(null);

    const numPort = Number(mcpPortInput);
    if (!Number.isInteger(numPort) || numPort < 1 || numPort > 65535) {
      setError('MCP HTTP Port must be an integer between 1 and 65535.');
      return;
    }

    bridge.setMcpConfig(mcpMode, numPort);
    setStatusMessage(`Desktop MCP settings saved (${mcpMode} on port ${numPort}).`);
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
      setStatusMessage('Copied local MCP Bearer token to clipboard.');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to copy local MCP token');
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
      setStatusMessage('Rotated local MCP Bearer token.');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to rotate local MCP token');
    } finally {
      setBridgeBusy(false);
    }
  };

  const handleCopySnippet = async (target: McpClientTarget, snippet: string) => {
    setError(null);
    try {
      await bridge.copyToClipboard(snippet);
      setCopiedTarget(target);
      setStatusMessage(`Copied ${target} MCP JSON configuration.`);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to copy MCP configuration');
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

      {/* Server Base URL */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-4">
        <div className="flex items-center gap-2">
          <Globe className="h-5 w-5 text-indigo-400" />
          <div>
            <h2 className="text-base font-semibold text-white">Server Base URL</h2>
            <p className="text-xs text-slate-400">
              Leave empty for Web same-origin relative paths, or set{' '}
              <span className="font-mono">{DEFAULTS.DEFAULT_LOCAL_SERVER_URL}</span> / custom HTTPS
              endpoint for Desktop. Runtime: <span className="font-mono">{bridge.runtime}</span>.
            </p>
          </div>
        </div>

        <form onSubmit={handleSaveServerUrl} className="flex flex-wrap items-end gap-3">
          <label className="min-w-[260px] flex-1 space-y-1.5 text-xs">
            <span className="font-medium text-slate-200">Backend Server URL</span>
            <input
              type="text"
              value={draftBaseUrl}
              onChange={(e) => setDraftBaseUrl(e.target.value)}
              placeholder={`Empty for relative, or ${DEFAULTS.DEFAULT_LOCAL_SERVER_URL}`}
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder:text-slate-500"
            />
          </label>
          <button
            type="submit"
            className="rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500"
          >
            Save Server URL
          </button>
        </form>
      </section>

      {/* Desktop MCP Guard & Local Port */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-4">
        <div className="flex items-center gap-2">
          <ServerCog className="h-5 w-5 text-cyan-400" />
          <div>
            <h2 className="text-base font-semibold text-white">Desktop Local MCP Server</h2>
            <p className="text-xs text-slate-400">
              Binds strictly to <span className="font-mono">{DEFAULTS.MCP_BIND_HOST}</span> and
              enforces the selected capability guard.
            </p>
          </div>
        </div>

        <form onSubmit={handleSaveMcp} className="space-y-4">
          <div className="grid gap-4 sm:grid-cols-2">
            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">MCP Access Mode</span>
              <select
                value={mcpMode}
                onChange={(e) => setMcpMode(e.target.value as McpAccessMode)}
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              >
                {MCP_MODE_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {opt.label} ({opt.value})
                  </option>
                ))}
              </select>
            </label>

            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">Local HTTP Port</span>
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
            {MCP_MODE_OPTIONS.find((opt) => opt.value === mcpMode)?.description}
          </p>

          <button
            type="submit"
            className="rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500"
          >
            Save MCP Settings
          </button>
        </form>

        {isDesktopBridgeAvailable && (
          <div className="mt-4 space-y-3 rounded-lg border border-slate-800 bg-slate-950/70 p-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex items-center gap-2">
                <span className="text-xs font-semibold uppercase tracking-wide text-slate-300">
                  Desktop Bridge Status
                </span>
                {bridgeStatus && (
                  <span
                    className={`rounded-full px-2 py-0.5 text-[11px] font-medium ${
                      bridgeStatus.running
                        ? 'border border-emerald-500/40 bg-emerald-500/15 text-emerald-300'
                        : 'border border-amber-500/40 bg-amber-500/15 text-amber-300'
                    }`}
                  >
                    {bridgeStatus.running ? 'Running' : 'Stopped'}
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
                    Copy Local Token
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
                    Rotate Token
                  </button>
                )}
                <button
                  type="button"
                  disabled={bridgeLoading || bridgeBusy}
                  onClick={() => void refreshBridgeStatus()}
                  className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-900 px-2.5 py-1.5 text-xs font-medium text-slate-300 transition hover:bg-slate-800 disabled:cursor-not-allowed disabled:opacity-60"
                >
                  <RefreshCw className={`h-3.5 w-3.5 ${bridgeLoading ? 'animate-spin' : ''}`} />
                  Refresh
                </button>
              </div>
            </div>

            {bridgeStatus ? (
              <div className="grid gap-2 text-xs text-slate-300 sm:grid-cols-3">
                <div>
                  <span className="text-slate-500">Endpoint: </span>
                  <span className="font-mono text-slate-200">{bridgeStatus.endpointUrl}</span>
                </div>
                <div>
                  <span className="text-slate-500">Mode: </span>
                  <span className="font-mono text-slate-200">{bridgeStatus.mode}</span>
                </div>
                <div>
                  <span className="text-slate-500">Token Hint: </span>
                  <span className="font-mono text-slate-200">{bridgeStatus.tokenHint}</span>
                </div>
              </div>
            ) : (
              <p className="text-xs text-slate-400">
                {bridgeLoading ? 'Loading Desktop MCP bridge status...' : 'Bridge status unavailable.'}
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
          <h2 className="text-base font-semibold text-white">MCP Client Config Snippets</h2>
          <p className="mt-0.5 text-xs text-slate-400">
            Ready-to-paste JSON snippets with <span className="font-mono">YOUR_MCP_TOKEN</span>{' '}
            placeholders (no real secrets included).
          </p>
        </div>

        <div className="grid gap-4 lg:grid-cols-2">
          {MCP_TARGET_META.map(({ target, label, hint }) => {
            const snippet = buildMcpClientSnippet(target, {
              mcpPort: parsedPort,
              serverBaseUrl: draftBaseUrl,
            });
            const isCopied = copiedTarget === target;

            return (
              <div
                key={target}
                className="flex flex-col rounded-lg border border-slate-800 bg-slate-950/70 p-4"
              >
                <div className="mb-2 flex items-center justify-between gap-2">
                  <div>
                    <h3 className="text-sm font-semibold text-white">{label}</h3>
                    <p className="text-[11px] text-slate-400">{hint}</p>
                  </div>
                  <button
                    type="button"
                    onClick={() => void handleCopySnippet(target, snippet)}
                    className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-800 px-2.5 py-1.5 text-xs font-medium text-slate-100 transition hover:bg-slate-700"
                  >
                    {isCopied ? (
                      <>
                        <Check className="h-3.5 w-3.5 text-emerald-400" />
                        Copied
                      </>
                    ) : (
                      <>
                        <Copy className="h-3.5 w-3.5" />
                        Copy JSON
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
