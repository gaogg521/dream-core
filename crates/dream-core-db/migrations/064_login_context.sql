ALTER TABLE users ADD COLUMN sso_only INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN sso_provider TEXT;
ALTER TABLE users ADD COLUMN last_login_method TEXT;
ALTER TABLE users ADD COLUMN last_login_client TEXT;
ALTER TABLE users ADD COLUMN last_login_ip TEXT;
ALTER TABLE users ADD COLUMN last_login_user_agent TEXT;
ALTER TABLE mfa_challenges ADD COLUMN auth_method TEXT;
CREATE TABLE IF NOT EXISTS user_login_events (
    id TEXT PRIMARY KEY, user_id TEXT NOT NULL, method TEXT NOT NULL,
    client TEXT NOT NULL, ip TEXT, user_agent TEXT, created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_user_login_events_user_time ON user_login_events(user_id, created_at DESC);
