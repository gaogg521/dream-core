-- one-sso 001: SSO provider configs + external identity bindings (MySQL port).
-- Shares the `_one_migrations` ledger with one-org/one-employee; entry names
-- carry the `sso_` prefix to keep the key space disjoint. Mirrors the SQLite
-- 001 final state re-expressed for MySQL 8.0.16+ (conventions: BIGINT
-- timestamps, utf8mb4_0900_as_cs, TINYINT(1) flags — see one-org 001).

CREATE TABLE IF NOT EXISTS one_sso_providers (
    provider   VARCHAR(255) PRIMARY KEY,
    enabled    TINYINT(1) NOT NULL DEFAULT 0,
    config     TEXT NOT NULL DEFAULT ('{}'),
    updated_at BIGINT NOT NULL,
    updated_by VARCHAR(255) NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_sso_identities (
    id           VARCHAR(255) PRIMARY KEY,
    provider     VARCHAR(255) NOT NULL,
    external_id  VARCHAR(255) NOT NULL,
    user_id      VARCHAR(255) NOT NULL,
    tenant_id    VARCHAR(255) NOT NULL DEFAULT 'default',
    last_seen_at BIGINT NULL,
    created_at   BIGINT NOT NULL,
    UNIQUE KEY uq_one_sso_identities_provider_external (provider, external_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_sso_identities_user ON one_sso_identities (user_id);
CREATE INDEX idx_one_sso_identities_provider ON one_sso_identities (provider, external_id);
