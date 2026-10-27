-- MCP catalogs must never be authorized by wildcard. Other resource types retain their existing policy.
DELETE FROM one_resource_grants WHERE resource_type = 'mcp' AND resource_id = '*';
