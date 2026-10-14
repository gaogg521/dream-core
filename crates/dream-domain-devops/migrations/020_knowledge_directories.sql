ALTER TABLE one_rag_libraries ADD COLUMN parent_id TEXT;
ALTER TABLE one_rag_libraries ADD COLUMN kind TEXT NOT NULL DEFAULT 'space';
ALTER TABLE one_rag_libraries ADD COLUMN scope TEXT NOT NULL DEFAULT 'org';
CREATE INDEX IF NOT EXISTS idx_rag_library_parent ON one_rag_libraries(parent_id);
