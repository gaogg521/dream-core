CREATE TABLE IF NOT EXISTS one_mcp_user_selection (
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    server_ids_json TEXT NOT NULL DEFAULT '[]',
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, user_id)
);
