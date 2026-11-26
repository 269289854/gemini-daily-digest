-- Per-user preferred digest/UI locale for bilingual LLM synthesis and notification rendering
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS preferred_locale TEXT NOT NULL DEFAULT 'en-US';
