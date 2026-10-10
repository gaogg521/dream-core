-- Pre-create the table before the immutable billing_009 migration. Its
-- SQLite-style TEXT primary/index keys are invalid on MySQL; IF NOT EXISTS
-- then skips that old CREATE while its index statements remain applicable.
CREATE TABLE IF NOT EXISTS one_conversation_audit_requests (
    id              VARCHAR(255) PRIMARY KEY,
    enterprise_id   VARCHAR(255) NOT NULL,
    tenant_id       VARCHAR(255) NOT NULL,
    target_user_id  VARCHAR(255) NOT NULL,
    conversation_id VARCHAR(255) NOT NULL,
    requested_by    VARCHAR(255) NOT NULL,
    requested_at    BIGINT NOT NULL,
    fulfilled_at    BIGINT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
