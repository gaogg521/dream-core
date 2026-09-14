-- Tenant-level IM pipelines (enterprise bot registration) and console settings
-- (appearance / risk / marketplace JSON). Secrets encrypted at rest.

CREATE TABLE IF NOT EXISTS one_im_pipelines (
    id               TEXT PRIMARY KEY NOT NULL,
    tenant_id        TEXT NOT NULL,
    platform         TEXT NOT NULL,
    name             TEXT NOT NULL,
    enabled          INTEGER NOT NULL DEFAULT 1,
    endpoint         TEXT,
    app_id           TEXT,
    secret_encrypted TEXT,
    extra_json       TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_one_im_pipelines_tenant ON one_im_pipelines(tenant_id);

CREATE TABLE IF NOT EXISTS one_console_settings (
    tenant_id         TEXT PRIMARY KEY NOT NULL,
    appearance_json   TEXT NOT NULL DEFAULT '{}',
    risk_json         TEXT NOT NULL DEFAULT '{}',
    marketplace_json  TEXT NOT NULL DEFAULT '{}',
    updated_at        INTEGER NOT NULL
);
