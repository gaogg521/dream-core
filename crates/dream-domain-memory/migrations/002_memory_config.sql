-- Enterprise memory config (P2-2 followups §A.6): per-tenant extraction
-- settings for the LLM-backed turn extractor. See the MySQL twin
-- (migrations_mysql/002_memory_config.sql) for the full doc comment.
--
-- SQLite port: same shape, dynamically typed columns.

CREATE TABLE IF NOT EXISTS one_memory_config (
    tenant_id            TEXT PRIMARY KEY NOT NULL,
    extraction_channel_id TEXT,
    extraction_model     TEXT,
    updated_at           INTEGER NOT NULL
);
