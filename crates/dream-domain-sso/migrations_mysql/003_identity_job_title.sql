-- one-sso 003: job title on the identity profile snapshot, alongside the
-- display_name/org_unit_path columns from 002. Only Feishu populates this
-- today (via its Contact API — see FeishuProvider::fetch_org_profile).
-- (MySQL port)
ALTER TABLE one_sso_identities ADD COLUMN job_title VARCHAR(255) NULL;
