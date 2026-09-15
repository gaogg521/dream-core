-- Link an API asset to the MCP registry row created by "publish as MCP".
ALTER TABLE one_api_assets ADD COLUMN published_mcp_id TEXT;
