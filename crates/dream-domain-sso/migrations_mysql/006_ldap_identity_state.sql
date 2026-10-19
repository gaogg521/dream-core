CREATE TABLE IF NOT EXISTS one_sso_directory_identity_state (
    identity_id VARCHAR(64) PRIMARY KEY,
    user_id VARCHAR(64) NOT NULL,
    source_disabled BIGINT NOT NULL DEFAULT 0,
    revocation_pending BIGINT NOT NULL DEFAULT 0,
    last_checked_at BIGINT NOT NULL,
    last_error VARCHAR(128),
    INDEX idx_sso_directory_identity_user(user_id, source_disabled)
);
