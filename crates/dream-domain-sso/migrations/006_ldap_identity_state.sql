-- Separate from the Feishu directory mirror and administrator-owned user status.
CREATE TABLE IF NOT EXISTS one_sso_directory_identity_state (
    identity_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    source_disabled INTEGER NOT NULL DEFAULT 0,
    revocation_pending INTEGER NOT NULL DEFAULT 0,
    last_checked_at INTEGER NOT NULL,
    last_error TEXT
);
CREATE INDEX IF NOT EXISTS idx_sso_directory_identity_user
    ON one_sso_directory_identity_state(user_id, source_disabled);
