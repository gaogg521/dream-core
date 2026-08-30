-- CI/CD-style pipelines with run history (MySQL port).
-- Trigger values: manual | push | schedule.
-- Run status mirrors standard CI conventions.
--
-- MySQL note: `trigger` is a MySQL reserved word and is backticked.
CREATE TABLE IF NOT EXISTS one_pipelines (
    id           VARCHAR(255) PRIMARY KEY,
    name         VARCHAR(255) NOT NULL,
    description  TEXT NULL,
    status       VARCHAR(16) NOT NULL DEFAULT 'active'
                 CHECK (status IN ('active', 'disabled')),
    `trigger`    VARCHAR(16) NOT NULL DEFAULT 'manual'
                 CHECK(`trigger` IN ('manual', 'push', 'schedule')),
    creator_id   VARCHAR(255) NOT NULL,
    creator_name VARCHAR(255) NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_pipelines_status ON one_pipelines (status, updated_at DESC);

CREATE TABLE IF NOT EXISTS one_pipeline_runs (
    id          VARCHAR(255) PRIMARY KEY,
    pipeline_id VARCHAR(255) NOT NULL,
    status      VARCHAR(16) NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending', 'running', 'success', 'failed', 'cancelled')),
    triggered_by VARCHAR(255) NULL,
    started_at  BIGINT NULL,
    finished_at BIGINT NULL,
    log         LONGTEXT NULL,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_pipeline_runs_pipeline ON one_pipeline_runs (pipeline_id, created_at DESC);
CREATE INDEX idx_one_pipeline_runs_status ON one_pipeline_runs (status);
