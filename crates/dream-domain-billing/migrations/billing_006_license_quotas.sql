-- billing_006: surface the E4 LicensePayload additions (quotas beyond seats,
-- per-module authorization, and the display/bookkeeping fields) on the
-- activation record the admin console reads back — see
-- `dream_domain_billing::license_key::LicensePayload` for what each of these
-- mirrors. All nullable / default-empty: an activation row written before
-- this migration reads back with every new column absent, same "unconstrained"
-- semantics as the payload fields themselves.
ALTER TABLE one_license_activation ADD COLUMN tenant_cap INTEGER;
ALTER TABLE one_license_activation ADD COLUMN agent_node_cap INTEGER;
ALTER TABLE one_license_activation ADD COLUMN cpu_cores_cap INTEGER;
ALTER TABLE one_license_activation ADD COLUMN memory_mb_cap INTEGER;
-- JSON array of `LicenseModuleGrant`, same shape as the signed payload field.
-- '[]' (not NULL) so every read can parse it unconditionally.
ALTER TABLE one_license_activation ADD COLUMN modules TEXT NOT NULL DEFAULT '[]';
ALTER TABLE one_license_activation ADD COLUMN serial TEXT;
ALTER TABLE one_license_activation ADD COLUMN app_id TEXT;
ALTER TABLE one_license_activation ADD COLUMN file_name TEXT;
