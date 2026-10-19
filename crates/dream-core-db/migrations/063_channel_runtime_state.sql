CREATE TABLE IF NOT EXISTS assistant_plugin_state (
    owner_user_id TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    state_key TEXT NOT NULL,
    encrypted_value TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (owner_user_id, plugin_id, state_key),
    FOREIGN KEY (owner_user_id, plugin_id) REFERENCES assistant_plugins(owner_user_id, id) ON DELETE CASCADE
);
