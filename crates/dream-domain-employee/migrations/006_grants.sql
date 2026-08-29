-- Resource-authorization matrix support for digital employees (align-openocta
-- §3, delivery-gaps T4 step 2). Purely additive: a fresh table, no columns
-- touched on `one_personal_agents`.
--
-- Unlike the four resource types `one_resource_grants` already covers, this
-- table's grants do NOT widen access beyond what a resource's own
-- scope/visibility grants — a shared digital employee no longer defaults to
-- "usable by the whole tenant" the moment `one_personal_agents.visibility`
-- becomes 'shared'. A row here is what makes it usable/manageable by a
-- subject other than its owner at all. `employee_id = '*'` means "every
-- shared digital employee in the tenant". `permission = 'manage'` implies
-- 'use' (a manager can also run it).
CREATE TABLE IF NOT EXISTS one_employee_grants (
    id           TEXT PRIMARY KEY,
    tenant_id    TEXT NOT NULL,
    subject_type TEXT NOT NULL,  -- 'member' | 'department' | 'scene'
    subject_id   TEXT NOT NULL,
    employee_id  TEXT NOT NULL,  -- a specific agent id, or '*'
    permission   TEXT NOT NULL,  -- 'use' | 'manage'
    granted_by   TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    UNIQUE(tenant_id, subject_type, subject_id, employee_id)
);

CREATE INDEX IF NOT EXISTS idx_one_employee_grants_lookup
    ON one_employee_grants(tenant_id, subject_type, subject_id);
