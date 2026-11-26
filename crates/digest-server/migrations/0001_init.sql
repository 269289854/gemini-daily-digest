-- Initial 14-table PostgreSQL 15-18 schema for gemini-daily-digest
-- Shared canonical pool (shared_public) + strict per-user isolation (user_private).

-- 1. Users
CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY,
    oidc_sub TEXT NOT NULL UNIQUE,
    email TEXT,
    display_name TEXT,
    role TEXT NOT NULL DEFAULT 'member',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_login_at TIMESTAMPTZ
);

-- 2. User AI Configs (encrypted API keys)
CREATE TABLE IF NOT EXISTS user_ai_configs (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    provider_label TEXT NOT NULL,
    base_url TEXT NOT NULL,
    api_key_enc TEXT,
    api_key_hint TEXT,
    model TEXT NOT NULL,
    system_prompt TEXT NOT NULL,
    temperature REAL NOT NULL,
    max_tokens INT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 3. Canonical Sources (shared_public fetched once; user_private isolated per owner)
CREATE TABLE IF NOT EXISTS canonical_sources (
    id UUID PRIMARY KEY,
    source_type TEXT NOT NULL,
    canonical_key TEXT NOT NULL,
    scope TEXT NOT NULL,
    scope_owner_id UUID REFERENCES users(id) ON DELETE CASCADE,
    default_name TEXT NOT NULL,
    config_json_enc TEXT,
    etag TEXT,
    last_modified TEXT,
    last_fetched_at TIMESTAMPTZ,
    next_poll_at TIMESTAMPTZ,
    last_status TEXT,
    last_error TEXT,
    consecutive_failures INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_canonical_sources_identity UNIQUE NULLS NOT DISTINCT (source_type, canonical_key, scope_owner_id),
    CONSTRAINT chk_canonical_sources_scope_owner CHECK (
        (scope = 'shared_public' AND scope_owner_id IS NULL)
        OR (scope = 'user_private' AND scope_owner_id IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_canonical_sources_scope_poll
    ON canonical_sources (scope, next_poll_at);

-- 4. User Subscriptions
CREATE TABLE IF NOT EXISTS user_subscriptions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    canonical_source_id UUID NOT NULL REFERENCES canonical_sources(id) ON DELETE CASCADE,
    custom_name TEXT NOT NULL,
    category TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_user_subscriptions_user_source UNIQUE (user_id, canonical_source_id)
);

CREATE INDEX IF NOT EXISTS idx_user_subscriptions_source_enabled
    ON user_subscriptions (canonical_source_id, enabled);

CREATE INDEX IF NOT EXISTS idx_user_subscriptions_user_enabled
    ON user_subscriptions (user_id, enabled);

-- 5. Canonical Items + PG 15-18 compatible search_tsv trigger & GIN index
CREATE TABLE IF NOT EXISTS canonical_items (
    id UUID PRIMARY KEY,
    canonical_source_id UUID NOT NULL REFERENCES canonical_sources(id) ON DELETE CASCADE,
    external_id TEXT,
    canonical_url TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    title TEXT NOT NULL,
    author TEXT,
    summary_raw TEXT,
    body_pruned BOOLEAN NOT NULL DEFAULT FALSE,
    published_at TIMESTAMPTZ,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    search_tsv TSVECTOR,
    CONSTRAINT uq_canonical_items_source_url UNIQUE (canonical_source_id, canonical_url),
    CONSTRAINT uq_canonical_items_source_hash UNIQUE (canonical_source_id, content_hash)
);

CREATE OR REPLACE FUNCTION canonical_items_update_search_tsv()
RETURNS TRIGGER AS $$
BEGIN
    NEW.search_tsv :=
        setweight(to_tsvector('simple', COALESCE(NEW.title, '')), 'A') ||
        setweight(to_tsvector('simple', COALESCE(NEW.author, '')), 'B') ||
        setweight(to_tsvector('simple', COALESCE(NEW.summary_raw, '')), 'C');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_canonical_items_search_tsv ON canonical_items;
CREATE TRIGGER trg_canonical_items_search_tsv
    BEFORE INSERT OR UPDATE OF title, author, summary_raw
    ON canonical_items
    FOR EACH ROW
    EXECUTE FUNCTION canonical_items_update_search_tsv();

CREATE INDEX IF NOT EXISTS idx_canonical_items_search_tsv
    ON canonical_items USING GIN (search_tsv);

CREATE INDEX IF NOT EXISTS idx_canonical_items_source_dates
    ON canonical_items (canonical_source_id, published_at DESC NULLS LAST, fetched_at DESC);

CREATE INDEX IF NOT EXISTS idx_canonical_items_unpruned_fetched_at
    ON canonical_items (fetched_at)
    WHERE body_pruned = FALSE;

-- 6. User Item Matches (per-user watchlist matching & urgent alert tracking)
CREATE TABLE IF NOT EXISTS user_item_matches (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_id UUID NOT NULL REFERENCES canonical_items(id) ON DELETE CASCADE,
    matched_keywords TEXT[] NOT NULL DEFAULT '{}',
    priority TEXT NOT NULL,
    importance_score INT NOT NULL DEFAULT 0,
    alerted_at TIMESTAMPTZ,
    CONSTRAINT uq_user_item_matches_user_item UNIQUE (user_id, item_id)
);

CREATE INDEX IF NOT EXISTS idx_user_item_matches_alerts
    ON user_item_matches (user_id, priority, alerted_at);

CREATE INDEX IF NOT EXISTS idx_user_item_matches_item_id
    ON user_item_matches (item_id);

-- 7. Daily Digests (per-user daily synthesis + input_fingerprint idempotency)
CREATE TABLE IF NOT EXISTS daily_digests (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    digest_date DATE NOT NULL,
    input_fingerprint TEXT NOT NULL,
    title TEXT NOT NULL,
    overview_markdown TEXT NOT NULL,
    overview_html TEXT NOT NULL,
    sections_json JSONB NOT NULL,
    item_count INT NOT NULL,
    highlighted_count INT NOT NULL,
    prompt_tokens INT NOT NULL DEFAULT 0,
    completion_tokens INT NOT NULL DEFAULT 0,
    model_used TEXT NOT NULL,
    degraded_fallback BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_daily_digests_user_date UNIQUE (user_id, digest_date)
);

CREATE INDEX IF NOT EXISTS idx_daily_digests_user_date_desc
    ON daily_digests (user_id, digest_date DESC);

-- 8. Source Daily Digests (independent per-source daily timelines)
CREATE TABLE IF NOT EXISTS source_daily_digests (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    digest_id UUID REFERENCES daily_digests(id) ON DELETE CASCADE,
    canonical_source_id UUID NOT NULL REFERENCES canonical_sources(id) ON DELETE CASCADE,
    digest_date DATE NOT NULL,
    input_fingerprint TEXT NOT NULL,
    summary_markdown TEXT NOT NULL,
    key_points_json JSONB NOT NULL,
    item_count INT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_source_daily_digests_user_source_date UNIQUE (user_id, canonical_source_id, digest_date)
);

CREATE INDEX IF NOT EXISTS idx_source_daily_digests_user_source_date_desc
    ON source_daily_digests (user_id, canonical_source_id, digest_date DESC);

-- 9. User Notification Channels (encrypted outbound channel configs)
CREATE TABLE IF NOT EXISTS user_notification_channels (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_type TEXT NOT NULL,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    config_enc TEXT NOT NULL,
    config_hint TEXT NOT NULL,
    trigger_on_daily BOOLEAN NOT NULL DEFAULT TRUE,
    trigger_on_alert BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_user_notification_channels_user_enabled
    ON user_notification_channels (user_id, enabled);

-- 10. User Schedules
CREATE TABLE IF NOT EXISTS user_schedules (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    timezone TEXT NOT NULL,
    daily_time TEXT NOT NULL,
    email_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    send_when_empty BOOLEAN NOT NULL DEFAULT FALSE,
    alert_scan_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    alert_interval_minutes INT NOT NULL DEFAULT 60,
    last_daily_run_date DATE,
    last_alert_scan_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_user_schedules_alert_scan
    ON user_schedules (alert_scan_enabled, last_alert_scan_at);

-- 11. User Watchlist Rules
CREATE TABLE IF NOT EXISTS user_watchlist_rules (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    keywords TEXT[] NOT NULL,
    exclude_keywords TEXT[] NOT NULL DEFAULT '{}',
    priority TEXT NOT NULL,
    notify_immediately BOOLEAN NOT NULL DEFAULT FALSE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_user_watchlist_rules_user_enabled
    ON user_watchlist_rules (user_id, enabled);

-- 12. Digest Runs
CREATE TABLE IF NOT EXISTS digest_runs (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    run_type TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ,
    items_fetched INT NOT NULL DEFAULT 0,
    items_new INT NOT NULL DEFAULT 0,
    prompt_tokens INT NOT NULL DEFAULT 0,
    completion_tokens INT NOT NULL DEFAULT 0,
    error_summary TEXT
);

CREATE INDEX IF NOT EXISTS idx_digest_runs_user_started_desc
    ON digest_runs (user_id, started_at DESC);

-- 13. Delivery Logs
CREATE TABLE IF NOT EXISTS delivery_logs (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES digest_runs(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    digest_id UUID REFERENCES daily_digests(id) ON DELETE SET NULL,
    channel_id UUID REFERENCES user_notification_channels(id) ON DELETE SET NULL,
    channel_type TEXT NOT NULL,
    status TEXT NOT NULL,
    error_message TEXT,
    sent_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_delivery_logs_user_sent_desc
    ON delivery_logs (user_id, sent_at DESC);

CREATE INDEX IF NOT EXISTS idx_delivery_logs_run_id
    ON delivery_logs (run_id);

-- 14. User Bookmarks (protects bookmarked items from 180-day body pruning)
CREATE TABLE IF NOT EXISTS user_bookmarks (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_id UUID NOT NULL REFERENCES canonical_items(id) ON DELETE CASCADE,
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_user_bookmarks_user_item UNIQUE (user_id, item_id)
);

CREATE INDEX IF NOT EXISTS idx_user_bookmarks_user_created_desc
    ON user_bookmarks (user_id, created_at DESC);
