-- Knowledge import sources: a folder in a registered file storage mirrored into
-- a company knowledge space, plus the bookkeeping that lets a re-run tell a
-- changed file from an untouched one and a deleted file from a listing error.
CREATE TABLE IF NOT EXISTS one_rag_import_sources (
    id                 TEXT    PRIMARY KEY NOT NULL,
    name               TEXT    NOT NULL,
    storage_config_id  TEXT    NOT NULL,
    prefix             TEXT    NOT NULL DEFAULT '',
    library_id         TEXT    NOT NULL,
    scope              TEXT    NOT NULL DEFAULT 'org',
    team_id            TEXT,
    visibility         TEXT    NOT NULL DEFAULT 'all',
    extensions         TEXT    NOT NULL DEFAULT '[]',
    schedule           TEXT    NOT NULL DEFAULT 'manual',
    enabled            INTEGER NOT NULL DEFAULT 1,
    created_by         TEXT    NOT NULL,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    last_run_at        INTEGER,
    last_report        TEXT,
    running_since      INTEGER
);

ALTER TABLE one_rag_documents ADD COLUMN source_id TEXT;
ALTER TABLE one_rag_documents ADD COLUMN source_key TEXT;
ALTER TABLE one_rag_documents ADD COLUMN source_signature TEXT;
ALTER TABLE one_rag_documents ADD COLUMN source_state TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_rag_documents_source_key
    ON one_rag_documents(source_id, source_key) WHERE source_id IS NOT NULL;
