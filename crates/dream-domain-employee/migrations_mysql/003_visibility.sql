-- one-employee 003: digital employee visibility (A1 L3 team-shared) (MySQL port).
-- 'private' (default) = owner-only, unchanged L1/L2 behavior.
-- 'shared' = visible to and usable by any member of the same tenant, so a
-- board requirement can be dispatched to a team-shared employee.
-- Only meaningful in enterprise tenants; in the 'default' personal tenant the
-- single operator only ever sees their own employees.

ALTER TABLE one_personal_agents ADD COLUMN visibility VARCHAR(16) NOT NULL DEFAULT 'private';
CREATE INDEX idx_one_personal_agents_shared
    ON one_personal_agents (tenant_id, visibility);
