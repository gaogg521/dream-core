-- Config vault (P1-5, align-openocta "配置项（config vault）"): reusable,
-- admin-managed configuration entries (API base_url, model parameters,
-- connection strings, ...) stored as named entries that skills/tools reference
-- through a template syntax instead of pasting the same secrets into every
-- skill body (MySQL port).
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
--
-- MySQL notes: `key` is a reserved word and is backticked everywhere it
-- appears; `name` in the (tenant_id, name) unique key is VARCHAR(191) to
-- stay well under the InnoDB index-key byte limit.
CREATE TABLE IF NOT EXISTS one_config_sets (
    id          VARCHAR(255) PRIMARY KEY NOT NULL,
    tenant_id   VARCHAR(255) NOT NULL,
    -- The alias consumers write into `{{config.<name>.<key>}}`. Unique per
    -- tenant: two sets with the same alias would make the reference syntax
    -- ambiguous.
    name        VARCHAR(191) NOT NULL,
    description TEXT NOT NULL DEFAULT (''),
    created_by  VARCHAR(255) NOT NULL,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL,
    UNIQUE KEY uq_one_config_sets_tenant_name (tenant_id, name)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_config_entries (
    id        VARCHAR(255) PRIMARY KEY NOT NULL,
    tenant_id VARCHAR(255) NOT NULL,
    set_id    VARCHAR(255) NOT NULL,
    `key`     VARCHAR(191) NOT NULL,
    -- Plaintext for `sensitive = 0`; ciphertext (encrypt_string) for
    -- `sensitive = 1`. Never logged. (`value` and `sensitive` are MySQL 8.0
    -- reserved words — backticked like `key`.)
    `value`     TEXT NOT NULL,
    `sensitive` TINYINT(1) NOT NULL DEFAULT 0,
    UNIQUE KEY uq_one_config_entries_set_key (set_id, `key`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_config_entries_set ON one_config_entries (set_id);
