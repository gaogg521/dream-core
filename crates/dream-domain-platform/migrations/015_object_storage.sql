-- platform 015: S3-compatible object storage configurations (企业「文件管理」).
--
-- Distinct from the personal file vault, which stores objects on the local
-- filesystem under `<data_dir>/file-vault`. This table is the tenant's
-- *external* buckets — MinIO in the bundled compose stack, or any S3-compatible
-- endpoint — that an admin registers, tests, and browses from the console.
--
-- `secret_access_key_encrypted` follows the same rule as the container
-- registry secret above it: encrypted at rest with the service's key, never
-- selected into a DTO. The DTO reports `hasSecret: bool`.
--
-- `config_key` is the operator-facing handle (the reference product calls it
-- 配置 Key) and is unique per tenant. Exactly one row per tenant may carry
-- `is_default = 1`; the service enforces that, because a partial unique index
-- on a boolean is not portable to MySQL.
CREATE TABLE IF NOT EXISTS one_object_storage_configs (
    id                          TEXT    PRIMARY KEY NOT NULL,
    tenant_id                   TEXT    NOT NULL DEFAULT 'default',
    config_key                  TEXT    NOT NULL,
    bucket                      TEXT    NOT NULL,
    endpoint                    TEXT    NOT NULL,
    region                      TEXT    NOT NULL DEFAULT 'us-east-1',
    -- Optional key prefix every listing is scoped to.
    prefix                      TEXT,
    access_key_id               TEXT,
    secret_access_key_encrypted TEXT,
    -- MinIO and most self-hosted gateways need path-style addressing.
    force_path_style            INTEGER NOT NULL DEFAULT 1,
    is_default                  INTEGER NOT NULL DEFAULT 0,
    created_by                  TEXT    NOT NULL,
    created_at                  INTEGER NOT NULL,
    updated_at                  INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_one_object_storage_tenant_key
    ON one_object_storage_configs(tenant_id, config_key);
