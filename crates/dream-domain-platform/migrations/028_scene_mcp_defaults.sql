CREATE TABLE IF NOT EXISTS one_scene_mcp_defaults (tenant_id TEXT NOT NULL, scene_id TEXT NOT NULL, context_key TEXT NOT NULL, applied_at BIGINT NOT NULL, PRIMARY KEY (tenant_id, scene_id));
