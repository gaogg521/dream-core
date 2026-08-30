-- P1-6 (align-openocta): API assets — imported Swagger / OpenAPI documents
-- whose endpoints an admin can browse here and publish into the skill
-- registry (see api_assets.rs) so member agents can call them through their
-- shell tools (MySQL port).
--
-- `spec` keeps the raw document verbatim for replay/audit; `endpoints` holds
-- the parsed summary array [{method, path, summary, operationId}] that the UI
-- renders without re-parsing the (possibly large) spec. Only JSON specs are
-- accepted today — YAML is a known limitation (the workspace has no
-- serde_yaml dependency; the client converts before importing).
-- LONGTEXT for `spec`: user-controlled documents regularly exceed MySQL
-- TEXT's 64 KiB cap.
--
-- `published_skill_id` is a deliberate addition to the P1-6 column list: the
-- skill registry's name uniqueness is global (D7), so "re-publish updates the
-- original entry" cannot safely be keyed by name — a name match could hijack
-- an unrelated skill row. The durable link on the asset side is the dedup key.
CREATE TABLE IF NOT EXISTS one_api_assets (
    id                 VARCHAR(255) PRIMARY KEY NOT NULL,
    tenant_id          VARCHAR(255) NOT NULL DEFAULT 'default',
    name               VARCHAR(255) NOT NULL,
    -- 'openapi' | 'swagger'
    source_format      VARCHAR(16) NOT NULL,
    title              VARCHAR(255) NULL,
    version            VARCHAR(64) NULL,
    base_url           TEXT NULL,
    spec               LONGTEXT NOT NULL,
    endpoints          LONGTEXT NOT NULL,
    imported_by        VARCHAR(255) NOT NULL,
    published_skill_id VARCHAR(255) NULL,
    created_at         BIGINT NOT NULL,
    updated_at         BIGINT NOT NULL,
    deleted_at         BIGINT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_api_assets_tenant_created
    ON one_api_assets (tenant_id, created_at DESC);
