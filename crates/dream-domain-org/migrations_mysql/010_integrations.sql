-- P2-1 integration connectors (reserved framework): per-(tenant, provider)
-- external-system connector config (GitHub / GitLab / Jira / Feishu, ...)
-- (MySQL port).
--
-- This is the "reserved adapter" pattern (mirrors one_smtp_config / the billing
-- payment provider): storing a row here does NOT perform any real sync — no
-- connector client is wired in. It lets an org admin fill in credentials ahead
-- of time so a real `IntegrationProvider` can be dropped in at the app layer
-- later without a schema change.
--
-- `config_json` holds NON-secret provider-specific fields (org / project /
-- repo / board id, ...); the credential (token / API key) lives encrypted in
-- `secret_encrypted` (same at-rest encryption as the SMTP password), never
-- returned to the client. `enabled` defaults off so a half-filled connector is
-- inert until the admin explicitly turns it on.
CREATE TABLE IF NOT EXISTS one_integrations (
    tenant_id        VARCHAR(255) NOT NULL,
    provider         VARCHAR(64) NOT NULL,
    base_url         TEXT NULL,
    config_json      TEXT NULL,
    secret_encrypted TEXT NULL,
    enabled          TINYINT(1) NOT NULL DEFAULT 0,
    updated_at       BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, provider)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_integrations_tenant ON one_integrations (tenant_id);
