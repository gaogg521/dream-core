-- MySQL twin of migrations/021_secure_export.sql.
ALTER TABLE one_security_policy ADD COLUMN export_watermark_enabled TINYINT(1) NOT NULL DEFAULT 0;
ALTER TABLE one_security_policy ADD COLUMN export_encryption_mode VARCHAR(16) NOT NULL DEFAULT 'optional';

CREATE TABLE IF NOT EXISTS one_watermark_seeds (
 id VARCHAR(64) PRIMARY KEY,
 enterprise_id VARCHAR(255) NOT NULL,
 tenant_id VARCHAR(255) NOT NULL,
 user_id VARCHAR(255) NOT NULL,
 machine_id VARCHAR(255) NOT NULL,
 key_enc TEXT NOT NULL,
 created_at BIGINT NOT NULL,
 expires_at BIGINT NOT NULL,
 INDEX idx_one_wm_seeds_user (user_id, machine_id, expires_at DESC)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_export_events (
 id VARCHAR(64) PRIMARY KEY,
 enterprise_id VARCHAR(255) NOT NULL,
 tenant_id VARCHAR(255) NOT NULL,
 user_id VARCHAR(255) NOT NULL,
 machine_id VARCHAR(255) NULL,
 kind VARCHAR(32) NOT NULL,
 encrypted TINYINT(1) NOT NULL DEFAULT 0,
 package_id VARCHAR(64) NULL,
 seed_id VARCHAR(64) NULL,
 file_names TEXT NOT NULL,
 total_bytes BIGINT NOT NULL DEFAULT 0,
 content_sha256 VARCHAR(64) NULL,
 client_ts BIGINT NOT NULL,
 created_at BIGINT NOT NULL,
 INDEX idx_one_export_events_tenant (tenant_id, created_at DESC),
 INDEX idx_one_export_events_seed (seed_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_secure_export_packages (
 id VARCHAR(64) PRIMARY KEY,
 enterprise_id VARCHAR(255) NOT NULL,
 tenant_id VARCHAR(255) NOT NULL,
 owner_user_id VARCHAR(255) NOT NULL,
 machine_id VARCHAR(255) NULL,
 dek_enc TEXT NOT NULL,
 recipient_scope VARCHAR(16) NOT NULL,
 recipient_user_ids TEXT NOT NULL,
 file_names TEXT NOT NULL,
 total_bytes BIGINT NOT NULL DEFAULT 0,
 content_sha256 VARCHAR(64) NULL,
 seed_id VARCHAR(64) NULL,
 created_at BIGINT NOT NULL,
 expires_at BIGINT NULL,
 revoked_at BIGINT NULL,
 revoked_by VARCHAR(255) NULL,
 INDEX idx_one_sec_pkg_tenant (tenant_id, created_at DESC),
 INDEX idx_one_sec_pkg_owner (owner_user_id, created_at DESC)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_secure_export_opens (
 id VARCHAR(64) PRIMARY KEY,
 package_id VARCHAR(64) NOT NULL,
 user_id VARCHAR(255) NOT NULL,
 machine_id VARCHAR(255) NULL,
 result VARCHAR(16) NOT NULL,
 reason VARCHAR(64) NULL,
 created_at BIGINT NOT NULL,
 INDEX idx_one_sec_opens_pkg (package_id, created_at DESC),
 INDEX idx_one_sec_opens_user (user_id, created_at DESC)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
