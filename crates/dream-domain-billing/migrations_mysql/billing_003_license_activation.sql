-- billing_003: offline license-key activation (MySQL port).
--
-- Before this, `tier` was a plain column any customer admin could PUT, so a
-- deployment could grant itself the top tier. Tier now comes from a
-- vendor-signed license key; this table records which key produced the current
-- entitlement so an operator can see (and support can audit) what is active.
--
-- Idempotency: `license_id` is the key's `lid` claim and is UNIQUE (the
-- primary key), so re-pasting the same key is a no-op rather than a duplicate
-- row.

CREATE TABLE IF NOT EXISTS one_license_activation (
    license_id    VARCHAR(255) PRIMARY KEY NOT NULL,
    enterprise_id VARCHAR(255) NOT NULL,
    customer      VARCHAR(255) NOT NULL,
    tier          VARCHAR(16) NOT NULL,
    seats         BIGINT NULL,
    expires_at    BIGINT NULL,
    issued_at     BIGINT NOT NULL,
    activated_at  BIGINT NOT NULL,
    activated_by  VARCHAR(255) NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_license_activation_enterprise
    ON one_license_activation (enterprise_id, activated_at DESC);
