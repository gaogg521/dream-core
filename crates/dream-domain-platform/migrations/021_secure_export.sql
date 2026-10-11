-- Export watermarking (A) and encrypted outbound packages (B). Design:
-- dream-en docs/design-2026-10-11-secure-export-and-watermark.zh-CN.md.
--
-- Enforcement lives in the member's desktop client: exports happen inside
-- Electron and never touch a backend that could gate them. This side issues
-- policy, watermark seeds and package keys, and keeps the audit trail.
--
-- `export_watermark_enabled`: stamp a signed, render-invisible line at the
--   end of every exported Markdown file.
-- `export_encryption_mode`: 'optional' (default; encryption is offered, not
--   preselected) | 'default' (preselected, can be unticked) | 'required'
--   (plaintext export is unavailable).
ALTER TABLE one_security_policy ADD COLUMN export_watermark_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE one_security_policy ADD COLUMN export_encryption_mode TEXT NOT NULL DEFAULT 'optional';

-- One HMAC key per (member, machine). `id` is what a watermark carries; it is
-- random and says nothing about the member until an admin looks it up.
CREATE TABLE IF NOT EXISTS one_watermark_seeds (
    id            TEXT    PRIMARY KEY,
    enterprise_id TEXT    NOT NULL,
    tenant_id     TEXT    NOT NULL,
    user_id       TEXT    NOT NULL,
    machine_id    TEXT    NOT NULL,
    -- `encv1:` envelope under the secure-export key; never returned to admins.
    key_enc       TEXT    NOT NULL,
    created_at    INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_one_wm_seeds_user ON one_watermark_seeds(user_id, machine_id, expires_at DESC);

-- What left the client, as the client reported it. No content, only names,
-- sizes and a hash. `id` is client-generated so a retried upload is a no-op.
CREATE TABLE IF NOT EXISTS one_export_events (
    id              TEXT    PRIMARY KEY,
    enterprise_id   TEXT    NOT NULL,
    tenant_id       TEXT    NOT NULL,
    user_id         TEXT    NOT NULL,
    machine_id      TEXT,
    kind            TEXT    NOT NULL,
    encrypted       INTEGER NOT NULL DEFAULT 0,
    package_id      TEXT,
    seed_id         TEXT,
    file_names      TEXT    NOT NULL DEFAULT '[]',
    total_bytes     INTEGER NOT NULL DEFAULT 0,
    content_sha256  TEXT,
    client_ts       INTEGER NOT NULL,
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_one_export_events_tenant ON one_export_events(tenant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_one_export_events_seed ON one_export_events(seed_id);

-- An encrypted package's key escrow and authorization row. The ciphertext
-- never reaches the server; `dek_enc` alone opens nothing.
CREATE TABLE IF NOT EXISTS one_secure_export_packages (
    id                  TEXT    PRIMARY KEY,
    enterprise_id       TEXT    NOT NULL,
    tenant_id           TEXT    NOT NULL,
    owner_user_id       TEXT    NOT NULL,
    machine_id          TEXT,
    dek_enc             TEXT    NOT NULL,
    -- 'enterprise' | 'tenant' | 'users'
    recipient_scope     TEXT    NOT NULL,
    recipient_user_ids  TEXT    NOT NULL DEFAULT '[]',
    file_names          TEXT    NOT NULL DEFAULT '[]',
    total_bytes         INTEGER NOT NULL DEFAULT 0,
    content_sha256      TEXT,
    seed_id             TEXT,
    created_at          INTEGER NOT NULL,
    expires_at          INTEGER,
    revoked_at          INTEGER,
    revoked_by          TEXT
);
CREATE INDEX IF NOT EXISTS idx_one_sec_pkg_tenant ON one_secure_export_packages(tenant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_one_sec_pkg_owner ON one_secure_export_packages(owner_user_id, created_at DESC);

-- Every attempt to open a package, granted or not.
CREATE TABLE IF NOT EXISTS one_secure_export_opens (
    id          TEXT    PRIMARY KEY,
    package_id  TEXT    NOT NULL,
    user_id     TEXT    NOT NULL,
    machine_id  TEXT,
    result      TEXT    NOT NULL,
    reason      TEXT,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_one_sec_opens_pkg ON one_secure_export_opens(package_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_one_sec_opens_user ON one_secure_export_opens(user_id, created_at DESC);
