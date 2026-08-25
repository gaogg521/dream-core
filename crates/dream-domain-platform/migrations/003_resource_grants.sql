-- Resource authorization matrix (E5): grant a member or a whole department
-- access to a specific skill / MCP server / digital employee / model channel,
-- instead of the coarser org-wide / team-wide `scope`+`visibility` columns
-- those four registries already have (dream-domain-devops). This table is
-- additive — nothing here changes what those columns already allow; a grant
-- here can only ever add reachability, never take it away, so shipping this
-- table has zero effect on any existing install until an admin starts using
-- it, and the four registries' own SQL predicates are untouched.
--
-- `resource_id = '*'` means "every current and future resource of this type"
-- — the escape hatch for "just give this department all skills" without
-- granting one row per skill.
--
-- Department grants are NOT expanded into per-member rows here; resolving
-- "does user X have resource Y" has to walk the department tree at read time
-- (`PlatformService::effective_resource_ids`) the same way `one_user_org`
-- ties a member to one department and `one_departments.parent_id` ties a
-- department to its ancestors. That keeps a department move/rename a
-- single-row change instead of a fan-out rewrite of every grant under it.
CREATE TABLE IF NOT EXISTS one_resource_grants (
    id            TEXT    PRIMARY KEY NOT NULL,
    tenant_id     TEXT    NOT NULL,
    subject_type  TEXT    NOT NULL,  -- 'member' | 'department'
    subject_id    TEXT    NOT NULL,  -- one_user_org.user_id | one_departments.id
    resource_type TEXT    NOT NULL,  -- 'skill' | 'mcp' | 'employee' | 'model_channel'
    resource_id   TEXT    NOT NULL,  -- specific resource id, or '*'
    granted_by    TEXT    NOT NULL,
    created_at    INTEGER NOT NULL,
    UNIQUE(tenant_id, subject_type, subject_id, resource_type, resource_id)
);

-- Listing a subject's grants (the matrix UI's row view).
CREATE INDEX IF NOT EXISTS idx_one_resource_grants_subject
    ON one_resource_grants(tenant_id, subject_type, subject_id);

-- Listing who holds a grant on one resource (the matrix UI's column view, and
-- "who can see this skill" audits).
CREATE INDEX IF NOT EXISTS idx_one_resource_grants_resource
    ON one_resource_grants(tenant_id, resource_type, resource_id);
