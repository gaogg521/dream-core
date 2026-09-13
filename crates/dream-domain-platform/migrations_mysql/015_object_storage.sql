-- platform 015: S3-compatible object storage configurations (MySQL port).
-- See the SQLite copy for the design notes. `config_key` is VARCHAR(191)
-- because it is part of a unique index and utf8mb4 caps an indexed key at
-- 3072 bytes.
CREATE TABLE IF NOT EXISTS one_object_storage_configs (
    id                          VARCHAR(255) PRIMARY KEY NOT NULL,
    tenant_id                   VARCHAR(255) NOT NULL DEFAULT 'default',
    config_key                  VARCHAR(191) NOT NULL,
    bucket                      VARCHAR(255) NOT NULL,
    endpoint                    VARCHAR(512) NOT NULL,
    region                      VARCHAR(64)  NOT NULL DEFAULT 'us-east-1',
    prefix                      VARCHAR(512) NULL,
    access_key_id               VARCHAR(255) NULL,
    secret_access_key_encrypted LONGTEXT     NULL,
    force_path_style            TINYINT(1)   NOT NULL DEFAULT 1,
    is_default                  TINYINT(1)   NOT NULL DEFAULT 0,
    created_by                  VARCHAR(255) NOT NULL,
    created_at                  BIGINT       NOT NULL,
    updated_at                  BIGINT       NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE UNIQUE INDEX idx_one_object_storage_tenant_key
    ON one_object_storage_configs (tenant_id, config_key);
