-- Employee grants join the resource matrix (the "two backends" fix).
--
-- `one_employee_grants` (dream-domain-employee 006) ran parallel to this
-- table for digital employees only, with two semantic differences that this
-- migration folds in:
--
--   * `resource_type = 'employee'` rows here now carry the grant in a
--     `permission` column ('use' | 'manage'; the other four types keep the
--     column at its 'use' default — reachability only, unchanged);
--   * employees are whitelist-shaped by design (a shared employee is NOT
--     usable tenant-wide without a row — 006's own doc comment), so unlike
--     the other four types their mode has no dial: the employee read path in
--     dream-domain-employee is always restrictive, and `grant_mode` answers
--     Restrictive for 'employee' regardless of any row here. Backfilling a
--     mode row for 'employee' would be a lie the enforcement ignores.
--
-- The backfill copies every grant over with a derived id (`remp_` + the old
-- id) and ignores duplicates. The source table stays in place, untouched and
-- no longer read or written — deleting it is a later migration's job once
-- nothing downstream wants the history.
--
-- Ordering: dream-core-app runs one-employee migrations before one-platform
-- ones, so `one_employee_grants` always exists by the time this runs. The
-- CREATE TABLE IF NOT EXISTS below is a no-op there (employee 006 made the
-- real table); it only matters for this crate's own test harness, which runs
-- this migration set alone — the backfill then reads an empty twin and
-- copies nothing, instead of dying on a missing table.

CREATE TABLE IF NOT EXISTS one_employee_grants (
    id            TEXT PRIMARY KEY NOT NULL,
    tenant_id     TEXT    NOT NULL,
    subject_type  TEXT    NOT NULL,
    subject_id    TEXT    NOT NULL,
    employee_id   TEXT    NOT NULL,
    permission    TEXT    NOT NULL,
    granted_by    TEXT    NOT NULL,
    created_at    INTEGER NOT NULL
);

ALTER TABLE one_resource_grants ADD COLUMN permission TEXT NOT NULL DEFAULT 'use';

INSERT OR IGNORE INTO one_resource_grants
    (id, tenant_id, subject_type, subject_id, resource_type, resource_id, granted_by, created_at, permission)
SELECT 'remp_' || id, tenant_id, subject_type, subject_id, 'employee', employee_id, granted_by, created_at, permission
FROM one_employee_grants;
