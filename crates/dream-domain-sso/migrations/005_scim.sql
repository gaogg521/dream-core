-- SCIM 2.0 inbound provisioning: local user/group mirror + bearer token hash.

CREATE TABLE IF NOT EXISTS one_scim_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    token_sha256 TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS one_scim_users (
    id TEXT PRIMARY KEY,
    external_id TEXT NOT NULL UNIQUE,
    user_id TEXT NOT NULL,
    user_name TEXT NOT NULL,
    display_name TEXT,
    active INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_one_scim_users_user_id ON one_scim_users(user_id);

CREATE TABLE IF NOT EXISTS one_scim_groups (
    id TEXT PRIMARY KEY,
    external_id TEXT UNIQUE,
    display_name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS one_scim_group_members (
    group_id TEXT NOT NULL,
    user_scim_id TEXT NOT NULL,
    PRIMARY KEY (group_id, user_scim_id)
);
