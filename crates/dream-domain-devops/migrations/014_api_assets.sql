-- P1-6 (align-openocta): API assets — imported Swagger / OpenAPI documents
-- whose endpoints an admin can browse here and publish into the skill
-- registry (see api_assets.rs) so member agents can call them through their
-- shell tools.
--
-- `spec` keeps the raw document verbatim for replay/audit; `endpoints` holds
-- the parsed summary array [{method, path, summary, operationId}] that the UI
-- renders without re-parsing the (possibly large) spec. Only JSON specs are
-- accepted today — YAML is a known limitation (the workspace has no
-- serde_yaml dependency; the client converts before importing).
--
-- `published_skill_id` is a deliberate addition to the P1-6 column list: the
-- skill registry's name uniqueness is global (D7), so "re-publish updates the
-- original entry" cannot safely be keyed by name — a name match could hijack
-- an unrelated skill row. The durable link on the asset side is the dedup key.
CREATE TABLE IF NOT EXISTS one_api_assets (
    id                 TEXT    PRIMARY KEY NOT NULL,
    tenant_id          TEXT    NOT NULL DEFAULT 'default',
    name               TEXT    NOT NULL,
    -- 'openapi' | 'swagger'
    source_format      TEXT    NOT NULL,
    title              TEXT,
    version            TEXT,
    base_url           TEXT,
    spec               TEXT    NOT NULL,
    endpoints          TEXT    NOT NULL,
    imported_by        TEXT    NOT NULL,
    published_skill_id TEXT,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    deleted_at         INTEGER
);

CREATE INDEX IF NOT EXISTS idx_one_api_assets_tenant_created
    ON one_api_assets(tenant_id, created_at DESC);
