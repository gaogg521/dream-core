-- Knowledge import sources (MySQL). See the SQLite file for the rationale.
CREATE TABLE IF NOT EXISTS one_rag_import_sources (
    id                 VARCHAR(64)  PRIMARY KEY NOT NULL,
    name               VARCHAR(255) NOT NULL,
    storage_config_id  VARCHAR(64)  NOT NULL,
    prefix             VARCHAR(1024) NOT NULL,
    library_id         VARCHAR(64)  NOT NULL,
    scope              VARCHAR(16)  NOT NULL DEFAULT 'org',
    team_id            VARCHAR(64)  NULL,
    visibility         VARCHAR(16)  NOT NULL DEFAULT 'all',
    extensions         TEXT         NOT NULL,
    schedule           VARCHAR(16)  NOT NULL DEFAULT 'manual',
    enabled            TINYINT(1)   NOT NULL DEFAULT 1,
    created_by         VARCHAR(64)  NOT NULL,
    created_at         BIGINT       NOT NULL,
    updated_at         BIGINT       NOT NULL,
    last_run_at        BIGINT       NULL,
    last_report        LONGTEXT     NULL,
    running_since      BIGINT       NULL
);

ALTER TABLE one_rag_documents ADD COLUMN source_id VARCHAR(64) NULL;
-- (64 + 512 chars) * 4 bytes stays inside InnoDB's 3072-byte index key limit.
ALTER TABLE one_rag_documents ADD COLUMN source_key VARCHAR(512) NULL;
ALTER TABLE one_rag_documents ADD COLUMN source_signature VARCHAR(255) NULL;
ALTER TABLE one_rag_documents ADD COLUMN source_state VARCHAR(16) NULL;

-- MySQL has no partial index; NULL source_id rows never collide in a unique index.
CREATE UNIQUE INDEX idx_rag_documents_source_key ON one_rag_documents(source_id, source_key);
