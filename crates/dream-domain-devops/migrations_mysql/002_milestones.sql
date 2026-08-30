-- Milestones: group requirements by a delivery target. one_requirements
-- already carries a milestone_id column (added in 001) that referenced
-- nothing; this table gives it a home. No FK constraint (SQLite soft link,
-- matching the rest of the one-* schema) — deleting a milestone leaves
-- requirements with a dangling milestone_id, cleared by the service layer
-- (MySQL port).
CREATE TABLE IF NOT EXISTS one_milestones (
    id           VARCHAR(255) PRIMARY KEY,
    title        VARCHAR(255) NOT NULL,
    description  TEXT NULL,
    status       VARCHAR(16) NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'completed', 'archived')),
    due_at       BIGINT NULL,
    creator_id   VARCHAR(255) NOT NULL,
    creator_name VARCHAR(255) NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_milestones_status ON one_milestones (status, updated_at DESC);
