-- Test plans: high-level plans linking test cases to a requirement milestone.
-- test cases hold individual test steps + result (MySQL port).
CREATE TABLE IF NOT EXISTS one_test_plans (
    id            VARCHAR(255) PRIMARY KEY,
    title         VARCHAR(255) NOT NULL,
    description   TEXT NULL,
    status        VARCHAR(16) NOT NULL DEFAULT 'draft'
                  CHECK (status IN ('draft', 'active', 'completed', 'archived')),
    requirement_id VARCHAR(255) NULL,
    creator_id    VARCHAR(255) NOT NULL,
    creator_name  VARCHAR(255) NULL,
    created_at    BIGINT NOT NULL,
    updated_at    BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_test_plans_req ON one_test_plans (requirement_id, updated_at DESC);
CREATE INDEX idx_one_test_plans_status ON one_test_plans (status, updated_at DESC);

CREATE TABLE IF NOT EXISTS one_test_cases (
    id           VARCHAR(255) PRIMARY KEY,
    plan_id      VARCHAR(255) NOT NULL,
    title        VARCHAR(255) NOT NULL,
    description  TEXT NULL,
    steps        TEXT NULL,   -- JSON string: [{action, expected}]
    expected     TEXT NULL,
    status       VARCHAR(16) NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'passed', 'failed', 'blocked', 'skipped')),
    creator_id   VARCHAR(255) NOT NULL,
    creator_name VARCHAR(255) NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_test_cases_plan ON one_test_cases (plan_id, updated_at DESC);
CREATE INDEX idx_one_test_cases_status ON one_test_cases (status);
