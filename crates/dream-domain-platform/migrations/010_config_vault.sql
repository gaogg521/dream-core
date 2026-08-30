-- Config vault (P1-5, align-openocta "配置项（config vault）"): reusable,
-- admin-managed configuration entries (API base_url, model parameters,
-- connection strings, ...) stored as named entries that skills/tools reference
-- through a template syntax instead of pasting the same secrets into every
-- skill body.
--
-- Two tables:
--
--   one_config_sets     a named configuration set — the alias referenced by
--                       consumers ("api", "model-params", ...). Unique per
--                       tenant so the reference syntax is unambiguous.
--   one_config_entries  the key/value payload of one set. `key` is unique
--                       within a set; `value` is TEXT, always non-null.
--
-- Sensitive entries (`sensitive = 1`) are encrypted at rest with the same
-- `encrypt_string` helper/key as the container registry_secret — the stored
-- bytes are ciphertext, never plaintext. Every read DTO redacts them: the
-- list/read surface only ever carries `hasValue: true` plus the key and a
-- "<sensitive>" placeholder, never the decrypted value (decryption exists as
-- a service method for future runtime consumers, not for any admin route).
--
-- Updating an entry with no value supplied keeps the stored one — the same
-- "absent = keep" convention as the container/collaboration/SIEM secrets,
-- so an admin can flip the sensitive flag (or rename nothing) without
-- re-pasting a credential they may not have in front of them.
--
-- Referencing consumers ("引用计数") are NOT tracked in a column. A set's
-- reference count is computed at read time by scanning the one table whose
-- content can embed the reference syntax `{{config.<set-alias>.<key>}}` —
-- devops' `one_skill_registry.content` (see PlatformService::
-- config_set_references for the query and its honest boundary notes).
-- Foreign keys between sets and entries are enforced by the application
-- layer (delete cascades in one transaction), matching this crate's other
-- tables; SQLite here runs without FK enforcement enabled.
CREATE TABLE IF NOT EXISTS one_config_sets (
    id          TEXT    PRIMARY KEY NOT NULL,
    tenant_id   TEXT    NOT NULL,
    -- The alias consumers write into `{{config.<name>.<key>}}`. Unique per
    -- tenant: two sets with the same alias would make the reference syntax
    -- ambiguous.
    name        TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    created_by  TEXT    NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    UNIQUE(tenant_id, name)
);

CREATE TABLE IF NOT EXISTS one_config_entries (
    id        TEXT    PRIMARY KEY NOT NULL,
    tenant_id TEXT    NOT NULL,
    set_id    TEXT    NOT NULL,
    key       TEXT    NOT NULL,
    -- Plaintext for `sensitive = 0`; ciphertext (encrypt_string) for
    -- `sensitive = 1`. Never logged.
    value     TEXT    NOT NULL,
    sensitive INTEGER NOT NULL DEFAULT 0,
    UNIQUE(set_id, key)
);

CREATE INDEX IF NOT EXISTS idx_one_config_entries_set ON one_config_entries(set_id);
