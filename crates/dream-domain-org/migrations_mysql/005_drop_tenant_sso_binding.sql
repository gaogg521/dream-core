-- Decouple the SSO company from project-group tenants (MySQL port).
--
-- The 07-16 "real enterprise = SSO-bound tenant" (B2) model tagged a tenant
-- with its SSO company (`sso_provider`/`sso_org_id`) and auto-joined
-- same-company logins into it. That conflated two orthogonal concepts. The SSO
-- company now lives in its own domain (one-enterprise: `one_enterprises` /
-- `one_enterprise_members`), so `one_tenants` goes back to being a pure
-- invite-code project group with no SSO binding.
--
-- MySQL 8.0 supports DROP INDEX and DROP COLUMN directly. Existing tenants
-- keep all their data; they just lose the (now unused) binding columns.
DROP INDEX idx_one_tenants_sso_org ON one_tenants;
ALTER TABLE one_tenants DROP COLUMN sso_provider;
ALTER TABLE one_tenants DROP COLUMN sso_org_id;
