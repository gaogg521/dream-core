ALTER TABLE one_llm_calls ADD COLUMN cache_read_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE one_llm_calls ADD COLUMN cache_write_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE one_llm_calls ADD COLUMN request_id TEXT;
ALTER TABLE one_llm_calls ADD COLUMN user_ip TEXT;
ALTER TABLE one_llm_calls ADD COLUMN credential_key_id TEXT;
CREATE INDEX IF NOT EXISTS idx_one_llm_calls_key ON one_llm_calls(enterprise_id, credential_key_id, created_at DESC);
