-- MySQL mirror of migrations/012_employee_grant_unification.sql. See that
-- file for the reasoning: the permission column, the employee backfill, and
-- why the source table is left in place.

CREATE TABLE IF NOT EXISTS one_employee_grants (
    id            VARCHAR(255) NOT NULL,
    tenant_id     VARCHAR(255) NOT NULL,
    subject_type  VARCHAR(255) NOT NULL,
    subject_id    VARCHAR(255) NOT NULL,
    employee_id   VARCHAR(255) NOT NULL,
    permission    VARCHAR(32)  NOT NULL,
    granted_by    VARCHAR(255) NOT NULL,
    created_at    BIGINT       NOT NULL,
    PRIMARY KEY (id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

ALTER TABLE one_resource_grants
    ADD COLUMN permission VARCHAR(32) NOT NULL DEFAULT 'use';

INSERT IGNORE INTO one_resource_grants
    (id, tenant_id, subject_type, subject_id, resource_type, resource_id, granted_by, created_at, permission)
SELECT CONCAT('remp_', id), tenant_id, subject_type, subject_id, 'employee', employee_id, granted_by, created_at, permission
FROM one_employee_grants;
