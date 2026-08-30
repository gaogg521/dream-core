-- Bind a tenant to the SSO company it represents, so an SSO login whose
-- company identifier (Feishu tenant_key / DingTalk corp_id / WeCom corpid)
-- matches can auto-join without an invite code (the "real enterprise" tier).
-- A tenant with a non-null sso_org_id is an SSO-company enterprise; a tenant
-- with a null sso_org_id is an invite-code project group. The two are mutually
-- exclusive per the "one server = one tenant" (D3) rule.
--
-- MySQL port: SQLite's partial unique index (`WHERE sso_org_id IS NOT NULL`)
-- maps to a plain unique index — MySQL unique indexes admit multiple NULL
-- pairs, which is exactly what the partial predicate carves out.
ALTER TABLE one_tenants ADD COLUMN sso_provider VARCHAR(64) NULL;
ALTER TABLE one_tenants ADD COLUMN sso_org_id VARCHAR(255) NULL;

CREATE UNIQUE INDEX idx_one_tenants_sso_org
    ON one_tenants (sso_provider, sso_org_id);
