-- one-org 001: enterprise tenant core tables (MySQL port of migrations/001_init.sql).
-- Managed by one-org's own migrator (_one_migrations), fully decoupled from
-- the upstream sqlx migrator (_sqlx_migrations) so upstream rebases never
-- conflict. Schema mirrors the SQLite 001 final state, re-expressed for
-- MySQL 8.0.16+.
--
-- MySQL port conventions (apply to every file in this directory):
-- - ms-epoch timestamps are BIGINT (SQLite INTEGER is 64-bit; MySQL INT is not).
-- - every table is InnoDB / utf8mb4 / utf8mb4_0900_as_cs — the case-sensitive
--   collation matches SQLite's semantics (a server-default `_ai_ci` collation
--   would make `WHERE name = 'API'` match 'api').
-- - SQLite's dynamically-typed TEXT stays TEXT unless the column is indexed
--   (MySQL indexes need bounded VARCHAR) or is a large free-form body (LONGTEXT).
-- - boolean-semantic INTEGER flags are TINYINT(1), decodable as both bool and
--   integer from the Rust side.
-- - SQLite does not enforce FOREIGN KEYs (no PRAGMA foreign_keys is ever set,
--   and startup migration explicitly runs with them off), so declared FKs are
--   carried as comments to keep MySQL behavior identical.

CREATE TABLE IF NOT EXISTS one_tenants (
    id                 VARCHAR(255) PRIMARY KEY,
    name               VARCHAR(255) NOT NULL,
    exit_password_hash TEXT NULL,
    created_at         BIGINT NOT NULL,
    updated_at         BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_tenant_invites (
    id         VARCHAR(255) PRIMARY KEY,
    tenant_id  VARCHAR(255) NOT NULL,
    code       VARCHAR(255) NOT NULL UNIQUE,
    created_by VARCHAR(255) NOT NULL,
    max_uses   BIGINT NULL,
    use_count  BIGINT NOT NULL DEFAULT 0,
    expires_at BIGINT NULL,
    created_at BIGINT NOT NULL,
    revoked    TINYINT(1) NOT NULL DEFAULT 0
    -- SQLite declared: FOREIGN KEY (tenant_id) REFERENCES one_tenants(id) ON DELETE CASCADE (unenforced)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_tenant_invites_tenant ON one_tenant_invites (tenant_id, created_at DESC);

-- Enterprise attributes for upstream users. No FK to the upstream `users`
-- table on purpose: upstream owns that table and we never ALTER it.
CREATE TABLE IF NOT EXISTS one_user_org (
    user_id              VARCHAR(255) NOT NULL,
    tenant_id            VARCHAR(255) NOT NULL,
    role                 VARCHAR(32) NOT NULL DEFAULT 'member',
    org_unit_path        TEXT NULL,
    org_profile_source   VARCHAR(64) NULL,
    org_profile_synced_at BIGINT NULL,
    created_at           BIGINT NOT NULL,
    updated_at           BIGINT NOT NULL
    -- SQLite declared a single-column PRIMARY KEY (user_id); 007_multi_membership
    -- rebuilds the table to (user_id, tenant_id) — applied by that file below.
    ,PRIMARY KEY (user_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_user_org_tenant ON one_user_org (tenant_id);

CREATE TABLE IF NOT EXISTS one_runtime_nodes (
    id               VARCHAR(255) PRIMARY KEY,
    tenant_id        VARCHAR(255) NOT NULL,
    user_id          VARCHAR(255) NOT NULL,
    machine_id       VARCHAR(255) NOT NULL,
    display_name     VARCHAR(255) NOT NULL,
    hostnames        TEXT NOT NULL DEFAULT ('[]'),
    ip_addresses     TEXT NOT NULL DEFAULT ('[]'),
    installed_agents TEXT NOT NULL DEFAULT ('[]'),
    last_seen_at     BIGINT NOT NULL,
    updated_at       BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_runtime_nodes_tenant_seen ON one_runtime_nodes (tenant_id, last_seen_at DESC);
CREATE INDEX idx_one_runtime_nodes_user ON one_runtime_nodes (tenant_id, user_id);

CREATE TABLE IF NOT EXISTS one_audit_logs (
    id         VARCHAR(255) PRIMARY KEY,
    tenant_id  VARCHAR(255) NOT NULL DEFAULT 'default',
    user_id    VARCHAR(255) NULL,
    username   VARCHAR(255) NULL,
    action     VARCHAR(255) NOT NULL,
    resource   TEXT NULL,
    ip_address VARCHAR(64) NULL,
    user_agent TEXT NULL,
    created_at BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_audit_tenant ON one_audit_logs (tenant_id, created_at DESC);
