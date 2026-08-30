-- one-org 009: organizational hierarchy (P2-3) — sub-teams / departments
-- within a project group (MySQL port). Distinct from
-- `one_user_org.org_unit_path` (a free-text SSO-synced department string,
-- e.g. Feishu's contact-book path): this is a structured, admin-managed tree
-- the operator builds by hand.
CREATE TABLE IF NOT EXISTS one_departments (
    id         VARCHAR(255) PRIMARY KEY NOT NULL,
    tenant_id  VARCHAR(255) NOT NULL,
    parent_id  VARCHAR(255) NULL,
    name       VARCHAR(255) NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
    -- SQLite declared: FOREIGN KEY (parent_id) REFERENCES one_departments(id)
    -- ON DELETE RESTRICT (unenforced — no PRAGMA foreign_keys)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_departments_tenant ON one_departments (tenant_id);
CREATE INDEX idx_one_departments_parent ON one_departments (parent_id);

-- A member's assigned department (nullable = unassigned, the default for
-- every existing row — zero behavior change).
ALTER TABLE one_user_org ADD COLUMN department_id VARCHAR(255) NULL;
