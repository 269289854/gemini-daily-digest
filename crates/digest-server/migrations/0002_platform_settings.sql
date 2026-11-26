-- Platform-wide singleton settings (encrypted admin integrations such as X Bridge / API).
CREATE TABLE IF NOT EXISTS platform_settings (
    id INT PRIMARY KEY CHECK (id = 1),
    x_bridge_base_url TEXT,
    x_bridge_reload_url TEXT,
    x_bridge_reload_secret_enc TEXT,
    x_bridge_reload_secret_hint TEXT,
    x_auth_token_enc TEXT,
    x_auth_token_hint TEXT,
    x_bearer_token_enc TEXT,
    x_bearer_token_hint TEXT,
    updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
