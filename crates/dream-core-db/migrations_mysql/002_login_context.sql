ALTER TABLE users ADD COLUMN sso_only BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE users ADD COLUMN sso_provider VARCHAR(32) NULL;
ALTER TABLE users ADD COLUMN last_login_method VARCHAR(32) NULL;
ALTER TABLE users ADD COLUMN last_login_client VARCHAR(32) NULL;
ALTER TABLE users ADD COLUMN last_login_ip VARCHAR(64) NULL;
ALTER TABLE users ADD COLUMN last_login_user_agent TEXT NULL;
CREATE TABLE IF NOT EXISTS user_login_events (
    id VARCHAR(64) PRIMARY KEY, user_id VARCHAR(255) NOT NULL, method VARCHAR(32) NOT NULL,
    client VARCHAR(32) NOT NULL, ip VARCHAR(64) NULL, user_agent TEXT NULL, created_at BIGINT NOT NULL,
    INDEX idx_user_login_events_user_time (user_id, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
