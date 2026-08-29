-- Personal file vault ("个人文件仓库", align-openocta P2-4): a per-member
-- file repository with an admin governance surface — availability status
-- (available / frozen), a per-member quota, usage aggregated from the
-- object ledger, and a reconciliation pass comparing the ledger against
-- what actually sits on disk.
--
-- Two tables:
--
--   one_file_vault_objects   the ledger: one row per stored file
--   one_file_vault_settings  per-member governance state (status + quota)
--
-- The settings row is created lazily on first touch — a member who never
-- stored a file and was never frozen has no row and reads as
-- "available / unlimited", which is exactly what the absence of a row means.
-- A frozen vault refuses new uploads but keeps existing objects readable and
-- deletable: freezing is a compliance hold on new data, not a lockdown of
-- the member's own historical files.
--
-- The actual bytes live under `<data_dir>/file-vault/<tenant>/<user>/`,
-- outside the database; the ledger row carries the relative storage key and
-- the file's SHA-256 so the reconcile pass can prove the two agree.
CREATE TABLE IF NOT EXISTS one_file_vault_objects (
    id           TEXT    PRIMARY KEY NOT NULL,
    tenant_id    TEXT    NOT NULL,
    user_id      TEXT    NOT NULL,
    file_name    TEXT    NOT NULL,
    size_bytes   INTEGER NOT NULL,
    sha256       TEXT    NOT NULL,
    -- Path relative to the vault storage root.
    storage_key  TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    deleted_at   INTEGER
);

CREATE INDEX IF NOT EXISTS idx_one_file_vault_objects_tenant_user
    ON one_file_vault_objects(tenant_id, user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS one_file_vault_settings (
    tenant_id    TEXT    NOT NULL,
    user_id      TEXT    NOT NULL,
    -- 'available' | 'frozen'
    status       TEXT    NOT NULL DEFAULT 'available',
    -- NULL = unlimited.
    quota_bytes  INTEGER,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, user_id)
);
