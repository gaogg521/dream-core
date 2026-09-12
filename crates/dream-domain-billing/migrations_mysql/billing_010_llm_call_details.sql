ALTER TABLE one_llm_calls ADD COLUMN cache_read_tokens BIGINT NOT NULL DEFAULT 0;
ALTER TABLE one_llm_calls ADD COLUMN cache_write_tokens BIGINT NOT NULL DEFAULT 0;
ALTER TABLE one_llm_calls ADD COLUMN request_id VARCHAR(255) NULL;
ALTER TABLE one_llm_calls ADD COLUMN user_ip VARCHAR(64) NULL;
ALTER TABLE one_llm_calls ADD COLUMN credential_key_id VARCHAR(255) NULL;
CREATE INDEX idx_one_llm_calls_key ON one_llm_calls(enterprise_id, credential_key_id, created_at DESC);
