-- How far the audit log has been forwarded to the company's SIEM (MySQL port).
-- See the SQLite migration for the contract.
CREATE TABLE IF NOT EXISTS one_siem_cursor (
    tenant_id       VARCHAR(255) PRIMARY KEY NOT NULL,
    last_created_at BIGINT       NOT NULL,
    last_id         VARCHAR(255) NOT NULL DEFAULT '',
    forwarded_total BIGINT       NOT NULL DEFAULT 0,
    last_run_at     BIGINT       NULL,
    last_status     VARCHAR(32)  NULL,
    last_error      TEXT         NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
