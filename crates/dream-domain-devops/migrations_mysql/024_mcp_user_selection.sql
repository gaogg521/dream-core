CREATE TABLE IF NOT EXISTS one_mcp_user_selection (
    tenant_id VARCHAR(255) NOT NULL,
    user_id VARCHAR(255) NOT NULL,
    server_ids_json TEXT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, user_id)
);
