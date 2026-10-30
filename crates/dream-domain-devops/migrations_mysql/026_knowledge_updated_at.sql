ALTER TABLE one_rag_documents ADD COLUMN updated_at BIGINT NOT NULL DEFAULT 0;
UPDATE one_rag_documents SET updated_at = created_at;
