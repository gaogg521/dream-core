-- Correct MySQL bootstrap for the historical 013 ledger entry.
-- Applied installs keep their ledger and schema; original shipped SQL stays immutable.
ALTER TABLE one_security_policy ADD COLUMN conversation_share_mode VARCHAR(16) NOT NULL DEFAULT 'off';
CREATE TABLE IF NOT EXISTS one_conversation_shares (
 id VARCHAR(255) PRIMARY KEY,
 conversation_id VARCHAR(255) NOT NULL UNIQUE,
 owner_user_id VARCHAR(255) NOT NULL,
 enterprise_id VARCHAR(255) NOT NULL,
 tenant_id VARCHAR(255) NOT NULL,
 scope VARCHAR(32) NOT NULL,
 name TEXT NOT NULL,
 uploaded TINYINT(1) NOT NULL DEFAULT 0,
 shared_at BIGINT NOT NULL,
 INDEX idx_one_conv_shares_ent (enterprise_id, scope, shared_at DESC),
 INDEX idx_one_conv_shares_tenant (tenant_id, shared_at DESC),
 INDEX idx_one_conv_shares_owner (owner_user_id, shared_at DESC)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
