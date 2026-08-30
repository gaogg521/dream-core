-- P1-4 security/compliance config: IP allowlist + SIEM audit-log export.
-- Same reserved-adapter shape as 001 (per-tenant singleton, credential
-- encrypted, enabled defaults off) (MySQL port).
--
-- IP allowlist: the CIDR ranges permitted to reach this project group's
-- server. The config + match logic are real, but request-blocking middleware is
-- NOT wired by default (it could lock out the operator) — a deployment enables
-- enforcement by dropping in the middleware. `cidrs` is a JSON array of strings.
CREATE TABLE IF NOT EXISTS one_ip_allowlist_config (
    tenant_id  VARCHAR(255) PRIMARY KEY NOT NULL,
    cidrs      TEXT NULL,          -- JSON array of CIDR / IP strings
    enabled    TINYINT(1) NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

-- SIEM export: forward audit logs to an external SIEM (Splunk HEC / syslog /
-- generic HTTP). Reserved adapter — no exporter ships; an export reports
-- "not configured" until a real `SiemExporter` is wired at the app layer.
CREATE TABLE IF NOT EXISTS one_siem_config (
    tenant_id        VARCHAR(255) PRIMARY KEY NOT NULL,
    kind             VARCHAR(32) NULL,  -- 'splunk' | 'syslog' | 'http' | 'none'
    endpoint         TEXT NULL,
    secret_encrypted TEXT NULL,         -- HEC token / auth header (encrypted)
    enabled          TINYINT(1) NOT NULL DEFAULT 0,
    updated_at       BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
