import { APP_ROUTES } from './constants/routes';
import { createPlatformBridge } from './platform/bridge';
import { McpAccessMode, SourceType } from './types/enums';

const bridge = createPlatformBridge();

export function App() {
  return (
    <main className="mx-auto flex min-h-screen max-w-4xl flex-col justify-center gap-6 px-6 py-12">
      <header className="space-y-2">
        <span className="inline-block rounded-full bg-indigo-500/20 px-3 py-1 text-xs font-medium text-indigo-300">
          Round 0 Foundation • Runtime: {bridge.runtime}
        </span>
        <h1 className="text-3xl font-bold tracking-tight text-white">Gemini Daily Digest</h1>
        <p className="text-slate-300">
          Multi-user self-hosted AI digest, per-account tracking, and MCP-ready workspace.
        </p>
      </header>
      <section className="grid gap-4 rounded-xl border border-slate-800 bg-slate-900/60 p-6 sm:grid-cols-2">
        <div>
          <h2 className="text-sm font-semibold text-slate-200">Registered Source Types</h2>
          <p className="mt-1 text-xs text-slate-400">{Object.values(SourceType).join(', ')}</p>
        </div>
        <div>
          <h2 className="text-sm font-semibold text-slate-200">Default MCP Guard & Routes</h2>
          <p className="mt-1 text-xs text-slate-400">
            Default: {McpAccessMode.READ_ONLY} • Home: {APP_ROUTES.DAILY}
          </p>
        </div>
      </section>
    </main>
  );
}
