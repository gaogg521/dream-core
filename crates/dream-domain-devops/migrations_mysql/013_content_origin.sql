-- P1-1 round 1 (align-openocta §4): skills and MCP tools gain origin/
-- category/publish metadata, mirroring dream-domain-employee's
-- 008_content_origin.sql for digital employees. `origin` reserves the
-- 'market' value for the not-yet-built remote-sync round. `published`
-- defaults to 1 (true) so every existing row stays visible to members who
-- could already see it -- this migration changes no read behavior by
-- itself, the WHERE-clause filtering lands separately in service.rs
-- (MySQL port).
ALTER TABLE one_skill_registry ADD COLUMN origin VARCHAR(16) NOT NULL DEFAULT 'self_built';
ALTER TABLE one_skill_registry ADD COLUMN category_id VARCHAR(255) NULL;
ALTER TABLE one_skill_registry ADD COLUMN published TINYINT(1) NOT NULL DEFAULT 1;

ALTER TABLE one_mcp_registry ADD COLUMN origin VARCHAR(16) NOT NULL DEFAULT 'self_built';
ALTER TABLE one_mcp_registry ADD COLUMN category_id VARCHAR(255) NULL;
ALTER TABLE one_mcp_registry ADD COLUMN published TINYINT(1) NOT NULL DEFAULT 1;
