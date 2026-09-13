-- one-devops 017: the manual-authoring half of API assets (MySQL port).
--
-- See the SQLite copy for why these columns exist. Two dialect notes:
--   - `code` is VARCHAR(191), not TEXT: it is part of a unique index, and
--     utf8mb4 caps an indexed key at 3072 bytes.
--   - MySQL has no partial indexes, so uniqueness is enforced on
--     (tenant_id, code) with NULL codes exempt — which is exactly MySQL's
--     own behaviour, since NULLs never collide in a unique index. The
--     soft-delete caveat that buys us: a deleted row keeps its code, so a
--     code cannot be reused after deletion. Callers get a clear 409 rather
--     than silently shadowing the tombstone.
ALTER TABLE one_api_assets ADD COLUMN code VARCHAR(191) NULL;
ALTER TABLE one_api_assets ADD COLUMN category VARCHAR(255) NULL;
ALTER TABLE one_api_assets ADD COLUMN tool_prefix VARCHAR(191) NULL;
ALTER TABLE one_api_assets ADD COLUMN auth_type VARCHAR(32) NOT NULL DEFAULT 'none';
ALTER TABLE one_api_assets ADD COLUMN auth_config LONGTEXT NULL;
ALTER TABLE one_api_assets ADD COLUMN enabled TINYINT(1) NOT NULL DEFAULT 1;
ALTER TABLE one_api_assets ADD COLUMN description TEXT NULL;

CREATE UNIQUE INDEX idx_one_api_assets_tenant_code
    ON one_api_assets (tenant_id, code);
