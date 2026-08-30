-- one-org 008: onboarding (P2-4) — domain-based auto-join + SMTP reservation
-- for invite emails (MySQL port).
--
-- `allowed_email_domains` on a tenant lets a new SSO login whose email domain
-- matches auto-join that project group without an invite code (admin opt-in;
-- NULL/empty = disabled, the default — existing tenants are unaffected).
ALTER TABLE one_tenants ADD COLUMN allowed_email_domains TEXT NULL;

-- Singleton SMTP configuration for sending invite emails. Reserved: no SMTP
-- client is wired in yet (see `EmailSender` trait in one-org) — an operator
-- with real SMTP credentials fills this in, and `enabled` flips on once a real
-- `EmailSender` is wired in the app layer. Until then every send is a no-op
-- with a clear "not configured" result, mirroring the payment-provider stub.
CREATE TABLE IF NOT EXISTS one_smtp_config (
    id                 BIGINT PRIMARY KEY CHECK (id = 1),
    host               VARCHAR(255) NULL,
    port               INT NULL,
    username           VARCHAR(255) NULL,
    -- Encrypted with the same `aionui_common::crypto` helper as other stored
    -- secrets (provider API keys, SSO client secrets).
    password_encrypted TEXT NULL,
    from_address       VARCHAR(255) NULL,
    enabled            TINYINT(1) NOT NULL DEFAULT 0,
    updated_at         BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
