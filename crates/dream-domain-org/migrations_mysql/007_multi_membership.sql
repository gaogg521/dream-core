-- one-org 007: multi-membership (一人多项目组) + per-user active tenant (MySQL port).
--
-- Phase 1 (企业三层) sidestepped the `one_user_org` PK = `user_id` constraint
-- (one user can only be in one project group) by "creating empty groups on
-- behalf of the company" without ever touching that PK. Phase 2 opens it up:
-- a real company member necessarily spans several project groups, so the PK
-- becomes `(user_id, tenant_id)` and each user carries an *active tenant*
-- (the project group currently in effect). `tenant_of(user_id)` /
-- `effective_role(user_id)` keep their signatures — they now resolve through
-- the active tenant instead of the single membership row — so the RBAC
-- extractors (OrgActor) and the team-resource TenantResolver need no change.
--
-- Personal / standalone edition is unaffected: it has no membership rows, so
-- `one_active_tenant` stays empty and every resolver falls back to the
-- DEFAULT_TENANT_ID / system_default_user path exactly as before.
--
-- The SQLite original rebuilds the table (copy into `one_user_org_new`, drop,
-- rename) because SQLite cannot alter a primary key. MySQL can: the data-
-- preserving equivalent is dropping the single-column PK and adding the
-- composite one in the same statement. Existing rows cannot violate the new
-- key (a superset of a unique key), so no backfill check is needed.

-- 1) Composite primary key on one_user_org. The 002/003 columns
--    (display_name, job_title) are already present on every row.
ALTER TABLE one_user_org
    DROP PRIMARY KEY,
    ADD PRIMARY KEY (user_id, tenant_id);

-- 2) Per-user active tenant (which project group a user is currently acting in).
CREATE TABLE IF NOT EXISTS one_active_tenant (
    user_id    VARCHAR(255) PRIMARY KEY,
    tenant_id  VARCHAR(255) NOT NULL,
    updated_at BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

-- 3) Backfill: every existing single membership becomes that user's active
--    tenant, so behaviour is byte-identical until a user joins a second group.
INSERT IGNORE INTO one_active_tenant (user_id, tenant_id, updated_at)
SELECT user_id, tenant_id, updated_at FROM one_user_org;
