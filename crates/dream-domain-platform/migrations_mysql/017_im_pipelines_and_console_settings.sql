-- Tenant-level IM pipelines and console settings (MySQL port).

CREATE TABLE IF NOT EXISTS one_im_pipelines (
    id               VARCHAR(64) PRIMARY KEY NOT NULL,
    tenant_id        VARCHAR(255) NOT NULL,
    platform         VARCHAR(64) NOT NULL,
    name             VARCHAR(255) NOT NULL,
    enabled          TINYINT(1) NOT NULL DEFAULT 1,
    endpoint         TEXT NULL,
    app_id           VARCHAR(255) NULL,
    secret_encrypted TEXT NULL,
    extra_json       TEXT NULL,
    created_at       BIGINT NOT NULL,
    updated_at       BIGINT NOT NULL,
    KEY idx_one_im_pipelines_tenant (tenant_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_console_settings (
    tenant_id         VARCHAR(255) PRIMARY KEY NOT NULL,
    appearance_json   TEXT NOT NULL,
    risk_json         TEXT NOT NULL,
    marketplace_json  TEXT NOT NULL,
    updated_at        BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
