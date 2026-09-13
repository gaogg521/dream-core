-- one-devops 017: the manual-authoring half of API assets.
--
-- 014 modelled an asset as "an imported OpenAPI document", so every column it
-- has is something `parse_spec` reads out of that document. The console also
-- needs the other entry point — an operator who types the asset in by hand,
-- because the upstream service has no published document — and that flow
-- carries fields no document supplies: a stable code, a category, the tool
-- prefix its published operations get named with, how to authenticate, and
-- whether the asset is live.
--
-- `auth_config` holds credentials (bearer tokens, API keys). It follows the
-- config-vault rule: write-only. It is never selected into a list or detail
-- payload — the DTO exposes `auth_configured: bool` instead — so a token
-- cannot leak through a page that merely renders the asset.
--
-- Every column is nullable or defaulted, so the rows 014 already wrote stay
-- valid: an imported asset simply has no code/category/prefix, `auth_type`
-- 'none', and is enabled.
ALTER TABLE one_api_assets ADD COLUMN code TEXT;
ALTER TABLE one_api_assets ADD COLUMN category TEXT;
ALTER TABLE one_api_assets ADD COLUMN tool_prefix TEXT;
ALTER TABLE one_api_assets ADD COLUMN auth_type TEXT NOT NULL DEFAULT 'none';
ALTER TABLE one_api_assets ADD COLUMN auth_config TEXT;
ALTER TABLE one_api_assets ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
ALTER TABLE one_api_assets ADD COLUMN description TEXT;

-- Codes are the operator-facing handle and the default tool prefix, so they
-- have to be unambiguous inside a tenant. Partial index: rows without a code
-- (everything 014 imported) do not collide with each other.
CREATE UNIQUE INDEX IF NOT EXISTS idx_one_api_assets_tenant_code
    ON one_api_assets(tenant_id, code)
    WHERE code IS NOT NULL AND deleted_at IS NULL;
