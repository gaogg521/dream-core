ALTER TABLE one_rag_libraries ADD COLUMN parent_id VARCHAR(64) NULL;
ALTER TABLE one_rag_libraries ADD COLUMN kind VARCHAR(16) NOT NULL DEFAULT 'space';
ALTER TABLE one_rag_libraries ADD COLUMN scope VARCHAR(16) NOT NULL DEFAULT 'org';
CREATE INDEX idx_rag_library_parent ON one_rag_libraries(parent_id);
