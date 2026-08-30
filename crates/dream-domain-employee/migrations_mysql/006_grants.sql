-- Resource-authorization matrix support for digital employees (align-openocta
-- §3, delivery-gaps T4 step 2). Purely additive: a fresh table, no columns
-- touched on `one_personal_agents` (MySQL port).
--
-- Unlike the four resource types `one_resource_grants` already covers, this
-- table's grants do NOT widen access beyond what a resource's own
-- scope/visibility grants — a shared digital employee no longer defaults to
-- "usable by the whole tenant" the moment `one_personal_agents.visibility`
-- becomes 'shared'. A row here is what makes it usable/manageable by a
-- subject other than its owner at all. `employee_id = '*'` means "every
-- shared digital employee in the tenant". `permission = 'manage'` implies
-- 'use' (a manager can also run it).
--
-- MySQL note: id columns are VARCHAR(191) so the 4-column unique key stays
-- under InnoDB's 3072-byte index limit with utf8mb4.
CREATE TABLE IF NOT EXISTS one_employee_grants (
    id           VARCHAR(191) PRIMARY KEY,
    tenant_id    VARCHAR(191) NOT NULL,
    subject_type VARCHAR(32) NOT NULL,  -- 'member' | 'department' | 'scene'
    subject_id   VARCHAR(191) NOT NULL,
    employee_id  VARCHAR(191) NOT NULL, -- a specific agent id, or '*'
    permission   VARCHAR(16) NOT NULL,  -- 'use' | 'manage'
    granted_by   VARCHAR(191) NOT NULL,
    created_at   BIGINT NOT NULL,
    UNIQUE KEY uq_one_employee_grants (tenant_id, subject_type, subject_id, employee_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_employee_grants_lookup
    ON one_employee_grants (tenant_id, subject_type, subject_id);
