-- P1-1 round 1: digital employees gain the same origin/category/publish
-- metadata skills and MCP tools get in dream-domain-devops's
-- 013_content_origin.sql. `origin` reserves the 'market' value for the
-- not-yet-built remote-sync round — this table never needs another
-- migration to accommodate it. `published` defaults to 1 (true) so every
-- existing row stays visible; this is purely additive, orthogonal to the
-- private/shared visibility model and the one_employee_grants matrix
-- (migration 006) — neither of those changes.
ALTER TABLE one_personal_agents ADD COLUMN origin TEXT NOT NULL DEFAULT 'self_built';
ALTER TABLE one_personal_agents ADD COLUMN category_id TEXT;
ALTER TABLE one_personal_agents ADD COLUMN published INTEGER NOT NULL DEFAULT 1;
