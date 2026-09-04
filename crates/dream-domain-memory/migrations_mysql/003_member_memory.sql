-- MySQL mirror of migrations/003_member_memory.sql. See that file for the
-- reasoning: member item authorship (deletion rights) and the per-member
-- recall opt-out, read fail-closed on the recall path.

ALTER TABLE one_memory_items
    ADD COLUMN author_user_id VARCHAR(255) NULL;

CREATE TABLE IF NOT EXISTS one_memory_member_prefs (
    tenant_id      VARCHAR(255) NOT NULL,
    user_id        VARCHAR(255) NOT NULL,
    recall_enabled TINYINT      NOT NULL DEFAULT 1,
    updated_at     BIGINT       NOT NULL,
    PRIMARY KEY (tenant_id, user_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
