-- How the resource matrix (003) is read, per tenant and per resource type
-- (MySQL port — see the SQLite file for the full rationale).
--
-- The matrix has always been additive: a grant can only ADD reachability on
-- top of each registry's own `scope`/`visibility` columns. That is not what an
-- administrator granting a department exactly three skills assumes, so this
-- table lets a tenant say "granted means ONLY the granted ones" per type.
--
-- Absent row = additive = today's behaviour, so no existing install changes
-- until an admin opts in.
--
-- SAFETY: every failure path on the way to reading this resolves to additive,
-- never restrictive — a transient database error must not blank out every
-- member's skill list.
--
-- MySQL note: the composite primary key is VARCHAR(191) + VARCHAR(32), well
-- under InnoDB's 3072-byte index limit with utf8mb4 ((191+32) × 4 = 892).
CREATE TABLE IF NOT EXISTS one_resource_grant_modes (
    tenant_id     VARCHAR(191) NOT NULL,
    resource_type VARCHAR(32)  NOT NULL, -- 'skill' | 'mcp' | 'knowledge' | 'model_channel'
    mode          VARCHAR(32)  NOT NULL, -- 'additive' | 'restrictive'
    updated_by    VARCHAR(191) NOT NULL,
    updated_at    BIGINT       NOT NULL,
    PRIMARY KEY (tenant_id, resource_type)
);
