# Gemini Daily Digest

An open-source, multi-user, self-hosted AI news aggregation and daily digest platform — built with **Rust** (`Axum` + `SQLx`), **PostgreSQL**, **React 19 + TypeScript**, **Tauri 2**, and a native **MCP Server** (`stdio` + local HTTP control bridge) for Gemini, ChatGPT, Claude, and Codex.

Out of the box it ships with a one-click **Gemini Ecosystem Preset Pack** (official blogs, changelogs, GitHub releases, Reddit, Hacker News, Google News, V2EX, and pluggable X/Twitter templates), while letting every authenticated user customize their own AI provider, monitored accounts/sources, notification channels, schedule, and alert rules in the UI.

## Architecture

```
External AI Clients (Gemini CLI / Antigravity / ChatGPT / Claude / Codex)
        │  MCP (stdio or 127.0.0.1 Streamable HTTP + Bearer Token)
        ▼
┌──────────────────────────────────────────────────────────────────────┐
│  Clients (Shared React SPA)                                          │
│  • Web Browser (OIDC PKCE)                                           │
│  • Tauri 2 Desktop (System Tray, Native Alerts, Loopback OIDC, MCP)  │
└───────────────────────────────┬──────────────────────────────────────┘
                                │ HTTPS + SSE (/api/v1/*)
                                ▼
┌──────────────────────────────────────────────────────────────────────┐
│  digest-server (Rust Axum + Tokio Dual Scheduler)                    │
│  • OIDC JWKS RS256 Auth (per-user isolation)                         │
│  • Per-user Daily Digest & High-Frequency Alert Scanner              │
│  • Pluggable Collectors, Pipeline Dedup, LLM Summarizer + Fallback   │
│  • SMTP & IM Notifiers (Gmail, Telegram, Feishu, DingTalk, WeCom)    │
└───────────────────────────────┬──────────────────────────────────────┘
                                │
                                ▼
                         PostgreSQL 18+
```

## Core Features

- **Multi-User Self-Service Configuration**: Each OIDC user manages their own OpenAI-compatible AI endpoint (e.g. DeepSeek, OpenAI, Gemini, Ollama), custom prompt, monitored sources/accounts, SMTP/IM channels, timezone, daily send time, and keyword watchlists.
- **Envelope Encryption & SSRF Protection**: User API keys, SMTP passwords, and webhook secrets are encrypted at rest using **AES-256-GCM** and never returned in plaintext. User-supplied URLs pass DNS-aware SSRF validation.
- **Dual-Dimension Dashboard**:
  - **Daily Overview**: AI executive summary, categorized cards, keyword highlight alerts, one-click Markdown copy, and original source links.
  - **Per-Source / Per-Account Timeline**: Inspect each monitored account's daily independent digest alongside its raw item feed.
  - **Full-Text Search & Bookmarks**: PostgreSQL `GIN` full-text search across historical items plus personal bookmarks and notes.
  - **Observability**: Run history, source health indicators, token consumption tracking, and one-click retry for failed deliveries.
- **Desktop Client + Built-in MCP Server**: Tauri 2 desktop app with system tray, native notifications via SSE, and a local `127.0.0.1` MCP server (`Disabled` / `ReadOnly` / `ReadWrite` permission guard, zero secret exposure).

## Engineering Charter

- **Zero Magic Values**: All protocol strings, status enums, HTTP headers, error codes, API routes, and tuning parameters live in centralized strongly typed enums, constants, and typed configuration structs.
- **Design Patterns**: Strategy + Registry (`SourceCollector`, `Notifier`), Decorator (`ResilientCollectorDecorator`), Pipeline (`ItemProcessingPipeline`), Template Method (`DigestExecutionWorkflow`), Repository (`Repository` traits), Command + Capability Guard (`McpDispatcher`), and Platform Adapter (`PlatformBridge`).
- **Strict Production Lint Policy**: `clippy::unwrap_used`, `clippy::expect_used`, and `clippy::panic` are denied in non-test Rust code.

## Quick Start (Development)

1. Copy `.env.example` to `.env` and set `DATABASE_URL` and `APP_MASTER_KEY_B64`.
2. Run Rust checks and tests:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
3. Run Web frontend checks:
   ```bash
   pnpm install
   pnpm --dir apps/web run typecheck
   pnpm --dir apps/web run test -- --run
   pnpm --dir apps/web run build
   ```

## Roadmap

- **Round 0**: Toolchain, repository scaffold, frozen `docs/API_CONTRACT.md`, foundational enums/constants/traits/MCP guard & Web bridge.
- **Round 1**: `digest-core` collectors, presets, crypto, SSRF guard, pipeline, LLM summarizer + fallback, HTML/Markdown renderers, SMTP & IM notifiers.
- **Round 2**: `digest-server` PostgreSQL migrations, repositories, OIDC JWKS auth, REST + SSE API, per-user dual scheduler.
- **Round 3**: `apps/web` full dashboard, per-source explorer, search/bookmarks, run stats, and self-service settings.
- **Round 4**: `digest-mcp` + `apps/desktop` Tauri 2 Windows app (tray, notifications, loopback OIDC, MCP bridge).
- **Round 5**: Container image, GitHub Actions CI/Release, and production deployment.

## License

[MIT](LICENSE)
