-- Resource authorization matrix (E5): grant a member or a whole department
-- access to a specific skill / MCP server / digital employee / model channel,
-- instead of the coarser org-wide / team-wide `scope`+`visibility` columns
-- those four registries already have (dream-domain-devops). This table is
-- additive — nothing here changes what those columns already allow; a grant
-- here can only ever add reachability, never take it away, so shipping this
-- table has zero effect on any existing install until an admin starts using
-- it, and the four registries' own SQL predicates are untouched (MySQL port).
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
--
-- MySQL note: id-ish columns here are VARCHAR(191) so the 5-column unique
-- key stays under InnoDB's 3072-byte index limit with utf8mb4
-- ((191+32+191+32+191) × 4 = 2548 bytes). Resource/subject ids are uuid-like
-- and tenant ids are short; 191 chars is far beyond anything real.
CREATE TABLE IF NOT EXISTS one_resource_grants (
    id            VARCHAR(191) PRIMARY KEY NOT NULL,
    tenant_id     VARCHAR(191) NOT NULL,
    subject_type  VARCHAR(32) NOT NULL,  -- 'member' | 'department'
    subject_id    VARCHAR(191) NOT NULL, -- one_user_org.user_id | one_departments.id
    resource_type VARCHAR(32) NOT NULL,  -- 'skill' | 'mcp' | 'employee' | 'model_channel'
    resource_id   VARCHAR(191) NOT NULL, -- specific resource id, or '*'
    granted_by    VARCHAR(191) NOT NULL,
    created_at    BIGINT NOT NULL,
    UNIQUE KEY uq_one_resource_grants (tenant_id, subject_type, subject_id, resource_type, resource_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

-- Listing a subject's grants (the matrix UI's row view).
CREATE INDEX idx_one_resource_grants_subject
    ON one_resource_grants (tenant_id, subject_type, subject_id);

-- Listing who holds a grant on one resource (the matrix UI's column view, and
-- "who can see this skill" audits).
CREATE INDEX idx_one_resource_grants_resource
    ON one_resource_grants (tenant_id, resource_type, resource_id);
