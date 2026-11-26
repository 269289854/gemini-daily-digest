# API, Database, MCP & Engineering Contract (Frozen in Round 0)

This document is the single source of truth across `digest-core`, `digest-server`, `digest-mcp`, `apps/web`, and `apps/desktop`.

---

## 1. Non-Negotiable Engineering Rules

1. **Zero Magic Values**:
   - All domain states, protocol strings, HTTP headers, error codes, SSE event names, MCP tool names, API routes, and tuning defaults MUST be referenced from `digest_core::domain::enums`, `digest_core::constants`, `digest_core::config`, and `apps/web/src/types/enums.ts` / `apps/web/src/constants/routes.ts`.
2. **Zero Secrets in Code or Repo**:
   - No real IPs, private domains, emails, API keys, or tokens may ever appear in source files, tests, fixtures, or docs.
3. **Production Error Hygiene**:
   - `clippy::unwrap_used`, `clippy::expect_used`, and `clippy::panic` are denied across all production crates. All errors map to `CoreError` -> `ErrorCode`.
4. **Row-Level Multi-User Isolation & Shared Upstream Deduplication**:
   - Public canonical sources (`scope = shared_public`, `scope_owner_id IS NULL`) are fetched **once** and stored **once** across all subscribers.
   - User-private sources (user-supplied credentials/X API keys, `scope = user_private`, `scope_owner_id = user_id`), user AI configs, notification channels, schedules, digests, and bookmarks are strictly isolated by `user_id`.

---

## 2. Shared Enums (Rust <-> TypeScript 1:1 Mirror)

- `SourceType` (`snake_case`): `rss`, `github_release`, `reddit`, `hacker_news`, `google_news`, `x_account`, `x_search`
- `SourceScope` (`snake_case`): `shared_public`, `user_private`
- `FetchOutcome` (`snake_case`): `updated`, `not_modified_304`, `skipped_fresh`, `failed`
- `NotificationChannelType` (`snake_case`): `smtp`, `telegram`, `feishu`, `dingtalk`, `wecom`, `webhook`
- `RunType` (`snake_case`): `scheduled_daily`, `manual_daily`, `alert_scan`
- `RunStatus` (`snake_case`): `running`, `succeeded`, `partial`, `failed`
- `DeliveryStatus` (`snake_case`): `pending`, `sent`, `failed`
- `AlertPriority` (`snake_case`): `normal`, `high`, `urgent`
- `McpAccessMode` (`snake_case`): `disabled`, `read_only`, `read_write`
- `AuthMode` (`snake_case`): `oidc`, `dev_bypass`
- `SseEventType` (`snake_case`): `run_progress`, `digest_ready`, `alert_triggered`, `heartbeat`
- `ErrorCode` (`SCREAMING_SNAKE_CASE`): `INVALID_INPUT`, `UNAUTHORIZED`, `FORBIDDEN`, `NOT_FOUND`, `CONFLICT`, `SSRF_BLOCKED`, `CRYPTO_ERROR`, `UPSTREAM_TIMEOUT`, `UPSTREAM_ERROR`, `DATABASE_ERROR`, `INTERNAL_ERROR`
- `McpToolName` (`snake_case`):
  - Read-only: `digest_get_daily`, `digest_list_dates`, `digest_list_sources`, `digest_get_source_digests`, `digest_search_items`, `digest_get_bookmarks`, `digest_get_runs_stats`
  - Read-write: `digest_trigger_run`, `digest_add_source`, `digest_toggle_source`, `digest_bookmark_item`, `digest_update_schedule`, `digest_add_watchlist_rule`, `digest_open_desktop_view`

---

## 3. PostgreSQL Schema (13 Tables — Shared Canonical Pool + User Isolation)

1. `users`: `id UUID PK`, `oidc_sub TEXT UNIQUE NOT NULL`, `email TEXT`, `display_name TEXT`, `role TEXT NOT NULL`, `created_at TIMESTAMPTZ`, `last_login_at TIMESTAMPTZ`
2. `user_ai_configs`: `user_id UUID PK FK -> users(id)`, `provider_label TEXT NOT NULL`, `base_url TEXT NOT NULL`, `api_key_enc TEXT`, `api_key_hint TEXT`, `model TEXT NOT NULL`, `system_prompt TEXT NOT NULL`, `temperature REAL NOT NULL`, `max_tokens INT NOT NULL`, `updated_at TIMESTAMPTZ`
3. `canonical_sources`: `id UUID PK`, `source_type TEXT NOT NULL`, `canonical_key TEXT NOT NULL`, `scope TEXT NOT NULL` (`shared_public` / `user_private`), `scope_owner_id UUID NULL FK -> users(id)`, `default_name TEXT NOT NULL`, `config_json_enc TEXT NULL`, `etag TEXT NULL`, `last_modified TEXT NULL`, `last_fetched_at TIMESTAMPTZ NULL`, `next_poll_at TIMESTAMPTZ NULL`, `last_status TEXT NULL`, `last_error TEXT NULL`, `consecutive_failures INT NOT NULL DEFAULT 0`, `created_at TIMESTAMPTZ NOT NULL`, `UNIQUE NULLS NOT DISTINCT (source_type, canonical_key, scope_owner_id)`
4. `user_subscriptions`: `id UUID PK`, `user_id UUID NOT NULL FK -> users(id)`, `canonical_source_id UUID NOT NULL FK -> canonical_sources(id)`, `custom_name TEXT NOT NULL`, `category TEXT NOT NULL`, `enabled BOOLEAN NOT NULL DEFAULT TRUE`, `created_at TIMESTAMPTZ NOT NULL`, `UNIQUE (user_id, canonical_source_id)`
5. `canonical_items`: `id UUID PK`, `canonical_source_id UUID NOT NULL FK -> canonical_sources(id)`, `external_id TEXT`, `canonical_url TEXT NOT NULL`, `content_hash TEXT NOT NULL`, `title TEXT NOT NULL`, `author TEXT`, `summary_raw TEXT`, `body_pruned BOOLEAN NOT NULL DEFAULT FALSE`, `published_at TIMESTAMPTZ`, `fetched_at TIMESTAMPTZ NOT NULL`, `search_tsv TSVECTOR` (GIN indexed), `UNIQUE (canonical_source_id, canonical_url)`, `UNIQUE (canonical_source_id, content_hash)`
6. `user_item_matches`: `id UUID PK`, `user_id UUID NOT NULL FK -> users(id)`, `item_id UUID NOT NULL FK -> canonical_items(id)`, `matched_keywords TEXT[] NOT NULL DEFAULT '{}'`, `priority TEXT NOT NULL`, `importance_score INT NOT NULL DEFAULT 0`, `alerted_at TIMESTAMPTZ NULL`, `UNIQUE (user_id, item_id)`
7. `daily_digests`: `id UUID PK`, `user_id UUID FK -> users(id)`, `digest_date DATE NOT NULL`, `input_fingerprint TEXT NOT NULL`, `title TEXT NOT NULL`, `overview_markdown TEXT NOT NULL`, `overview_html TEXT NOT NULL`, `sections_json JSONB NOT NULL`, `item_count INT NOT NULL`, `highlighted_count INT NOT NULL`, `prompt_tokens INT NOT NULL DEFAULT 0`, `completion_tokens INT NOT NULL DEFAULT 0`, `model_used TEXT NOT NULL`, `degraded_fallback BOOLEAN NOT NULL DEFAULT FALSE`, `created_at TIMESTAMPTZ NOT NULL`, `UNIQUE(user_id, digest_date)`
8. `source_daily_digests`: `id UUID PK`, `user_id UUID FK -> users(id)`, `digest_id UUID FK -> daily_digests(id)`, `canonical_source_id UUID FK -> canonical_sources(id)`, `digest_date DATE NOT NULL`, `input_fingerprint TEXT NOT NULL`, `summary_markdown TEXT NOT NULL`, `key_points_json JSONB NOT NULL`, `item_count INT NOT NULL`, `created_at TIMESTAMPTZ NOT NULL`, `UNIQUE(user_id, canonical_source_id, digest_date)`
9. `user_notification_channels`: `id UUID PK`, `user_id UUID FK -> users(id)`, `channel_type TEXT NOT NULL`, `name TEXT NOT NULL`, `enabled BOOLEAN NOT NULL DEFAULT TRUE`, `config_enc TEXT NOT NULL`, `config_hint TEXT NOT NULL`, `trigger_on_daily BOOLEAN NOT NULL DEFAULT TRUE`, `trigger_on_alert BOOLEAN NOT NULL DEFAULT TRUE`, `created_at TIMESTAMPTZ`
10. `user_schedules`: `user_id UUID PK FK -> users(id)`, `timezone TEXT NOT NULL`, `daily_time TEXT NOT NULL`, `email_enabled BOOLEAN NOT NULL DEFAULT TRUE`, `send_when_empty BOOLEAN NOT NULL DEFAULT FALSE`, `alert_scan_enabled BOOLEAN NOT NULL DEFAULT TRUE`, `alert_interval_minutes INT NOT NULL DEFAULT 60`, `last_daily_run_date DATE`, `last_alert_scan_at TIMESTAMPTZ`
11. `user_watchlist_rules`: `id UUID PK`, `user_id UUID FK -> users(id)`, `name TEXT NOT NULL`, `keywords TEXT[] NOT NULL`, `exclude_keywords TEXT[] NOT NULL DEFAULT '{}'`, `priority TEXT NOT NULL`, `notify_immediately BOOLEAN NOT NULL DEFAULT FALSE`, `enabled BOOLEAN NOT NULL DEFAULT TRUE`, `created_at TIMESTAMPTZ`
12. `digest_runs`: `id UUID PK`, `user_id UUID FK -> users(id)`, `run_type TEXT NOT NULL`, `status TEXT NOT NULL`, `started_at TIMESTAMPTZ NOT NULL`, `finished_at TIMESTAMPTZ`, `items_fetched INT NOT NULL DEFAULT 0`, `items_new INT NOT NULL DEFAULT 0`, `prompt_tokens INT NOT NULL DEFAULT 0`, `completion_tokens INT NOT NULL DEFAULT 0`, `error_summary TEXT`
13. `delivery_logs`: `id UUID PK`, `run_id UUID FK -> digest_runs(id)`, `user_id UUID FK -> users(id)`, `digest_id UUID FK -> daily_digests(id)`, `channel_id UUID FK -> user_notification_channels(id)`, `channel_type TEXT NOT NULL`, `status TEXT NOT NULL`, `error_message TEXT`, `sent_at TIMESTAMPTZ NOT NULL`
14. `user_bookmarks`: `id UUID PK`, `user_id UUID FK -> users(id)`, `item_id UUID FK -> canonical_items(id)`, `note TEXT`, `created_at TIMESTAMPTZ NOT NULL`, `UNIQUE(user_id, item_id)`

---

## 4. JSON Conventions & Unified Error Envelope

- All JSON request/response payloads MUST use **`camelCase`** field names (`#[serde(rename_all = "camelCase")]`).
- Unified error envelope:
  ```json
  {
    "code": "INVALID_INPUT",
    "message": "Human-readable error description without secrets",
    "requestId": "uuid-v4"
  }
  ```

---

## 5. REST & SSE Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/health` | Unauthenticated service health probe |
| `GET` | `/api/v1/auth/config` | Public OIDC config (`authMode`, `issuerUrl`, `clientId`, `scopes`) |
| `GET` | `/api/v1/me` | Current user profile & onboarding completion flags |
| `GET` | `/api/v1/digests` | Paginated daily digest summaries |
| `GET` | `/api/v1/digests/{date}` | Full daily digest for `YYYY-MM-DD` |
| `GET` | `/api/v1/digests/{date}/markdown` | Raw Markdown export for `YYYY-MM-DD` |
| `GET` | `/api/v1/sources/{id}/digests` | Daily independent digest timeline for one subscribed source/account |
| `GET` | `/api/v1/sources/{id}/items` | Paginated raw collected items for one subscribed source/account |
| `GET` | `/api/v1/items` | Full-text search (`q`, `sourceId`, `dateFrom`, `dateTo`, `keyword`, `bookmarked`, `page`, `pageSize`) |
| `POST` | `/api/v1/bookmarks` | Bookmark an item with optional note |
| `DELETE` | `/api/v1/bookmarks/{itemId}` | Remove bookmark |
| `GET` / `PUT` | `/api/v1/settings/ai` | Read (masked) or update user AI config |
| `POST` | `/api/v1/settings/ai/test` | Test AI endpoint connectivity |
| `GET` / `POST` | `/api/v1/settings/sources` | List or subscribe to a canonical source/account |
| `PUT` / `DELETE` | `/api/v1/settings/sources/{id}` | Update or unsubscribe from a source/account |
| `POST` | `/api/v1/settings/sources/import-preset` | Import built-in Gemini Ecosystem Preset Pack |
| `POST` | `/api/v1/settings/sources/export` | Export non-secret source definitions JSON |
| `POST` | `/api/v1/settings/sources/import` | Import source definitions JSON |
| `POST` | `/api/v1/settings/sources/{id}/test` | Live-fetch single source for immediate preview |
| `GET` / `POST` | `/api/v1/settings/channels` | List (masked) or create notification channel |
| `PUT` / `DELETE` | `/api/v1/settings/channels/{id}` | Update or delete notification channel |
| `POST` | `/api/v1/settings/channels/{id}/test` | Send test notification |
| `GET` / `PUT` | `/api/v1/settings/schedule` | Read or update user schedule and email master switch |
| `GET` / `POST` | `/api/v1/settings/watchlist` | List or create keyword watchlist alert rule |
| `PUT` / `DELETE` | `/api/v1/settings/watchlist/{id}` | Update or delete keyword watchlist alert rule |
| `POST` | `/api/v1/runs/trigger` | Trigger immediate fetch + summarize (+ optional notify) |
| `GET` | `/api/v1/runs` | List execution runs and delivery logs |
| `GET` | `/api/v1/runs/stats` | Token usage, source health, and delivery success metrics |
| `POST` | `/api/v1/deliveries/{id}/resend` | Re-dispatch a failed delivery log |
| `GET` | `/api/v1/events/stream` | Authenticated SSE stream (`run_progress`, `digest_ready`, `alert_triggered`, `heartbeat`) |

---

## 6. MCP Tools & Capability Guard Contract

- Transports:
  1. Standalone `stdio` binary (`digest-mcp`) for Gemini CLI / Antigravity / Claude Desktop / Codex.
  2. Desktop-hosted local Streamable HTTP MCP + REST Control Bridge bound strictly to `127.0.0.1:<port>` with random local Bearer token.
- Security invariant: **NO MCP tool may return decrypted AI keys, SMTP passwords, webhook tokens, or OIDC tokens.**

| Tool Name (`McpToolName`) | Required Access (`McpAccessMode`) | Summary |
| --- | --- | --- |
| `digest_get_daily` | `read_only` | Fetch full digest for a given date (or latest) |
| `digest_list_dates` | `read_only` | List available digest dates |
| `digest_list_sources` | `read_only` | List subscribed sources/accounts and health status |
| `digest_get_source_digests` | `read_only` | Get daily independent summaries & items for a source/account |
| `digest_search_items` | `read_only` | Full-text search across collected items |
| `digest_get_bookmarks` | `read_only` | List bookmarked items and notes |
| `digest_get_runs_stats` | `read_only` | Get run status, source health, and token stats |
| `digest_trigger_run` | `read_write` | Trigger a fetch/summarize run |
| `digest_add_source` | `read_write` | Subscribe to a new canonical source/account |
| `digest_toggle_source` | `read_write` | Enable or pause a subscribed source/account |
| `digest_bookmark_item` | `read_write` | Bookmark/unbookmark an item |
| `digest_update_schedule` | `read_write` | Update daily send time, timezone, or email switch |
| `digest_add_watchlist_rule` | `read_write` | Create a keyword highlight/alert rule |
| `digest_open_desktop_view` | `read_write` | Focus desktop window on a specific date/source/search view |

---

## 7. NIO Multiplexing, Resource Hard Limits & CacheCoordinator SPI

1. **Non-blocking NIO Event Loop (`tokio` + `mio` `epoll`/`IOCP`)**:
   - Runtime thread count explicitly capped via `TOKIO_WORKER_THREADS` (default `2`).
   - All network I/O (HTTP/2 collectors, PostgreSQL, Redis, SMTP, IM webhooks, SSE) runs purely async; CPU-heavy parsing/hashing/crypto runs in `tokio::task::spawn_blocking`.
2. **HTTP/2 Connection Multiplexing & Streaming Memory Cap**:
   - Single global `reqwest::Client` (HTTP/2 multiplexing + Keep-Alive + `gzip`/`brotli` compression).
   - Collector responses are streamed and aborted once exceeding `MAX_FEED_BYTES` (default `2 MiB`) to prevent OOM.
   - Bounded concurrency via `futures::stream::buffer_unordered` + global `Semaphore` (`GLOBAL_FETCH_CONCURRENCY`, default `4`).
3. **Bandwidth-Saving Adaptive Polling**:
   - Zero-subscriber canonical sources sleep with zero network/CPU cost.
   - Sources with alert-enabled subscribers poll periodically; daily-only sources fetch on-demand near schedule windows or manual triggers when stale beyond `SOURCE_FRESHNESS_TTL_SECS` (default `900s`).
   - All HTTP collectors send `If-None-Match` (`ETag`) and `If-Modified-Since` and short-circuit on `304 Not Modified`.
   - Failing sources apply exponential backoff (`1h -> 4h -> 12h -> 24h`).
4. **Pluggable `CacheCoordinator` Strategy**:
   - `RedisCacheCoordinator` (single multiplexed `redis::aio::ConnectionManager` connection when `REDIS_URL` is configured, e.g. Redis DB 2) provides singleflight locks and freshness cache.
   - `InMemoryCacheCoordinator` acts as zero-dependency fallback when `REDIS_URL` is unset.
5. **Database Pool & Long-Term Storage Hygiene**:
   - `sqlx::PgPool` bounded to `PG_MAX_CONNECTIONS` (default `5`).
   - Background maintenance prunes unbookmarked `canonical_items.summary_raw` older than `ITEM_BODY_RETENTION_DAYS` (default `180`) while preserving titles, canonical URLs, and daily digests.
