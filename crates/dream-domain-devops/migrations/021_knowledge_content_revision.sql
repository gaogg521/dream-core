ALTER TABLE one_rag_documents ADD COLUMN content_revision BIGINT NOT NULL DEFAULT 0;
UPDATE one_rag_documents SET status = 'ready' WHERE status = 'pending' AND TRIM(COALESCE(content, '')) = '';
