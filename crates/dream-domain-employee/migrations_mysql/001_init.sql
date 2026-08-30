-- one-employee 001: digital employee definitions + structured run history
-- (MySQL port). Shares the `_one_migrations` ledger with one-org (names are
-- prefixed `employee_` to keep the key space disjoint). Mirrors the SQLite
-- 001 final state re-expressed for MySQL 8.0.16+.

CREATE TABLE IF NOT EXISTS one_personal_agents (
    id                VARCHAR(255) PRIMARY KEY,
    owner_user_id     VARCHAR(255) NOT NULL,
    tenant_id         VARCHAR(255) NOT NULL DEFAULT 'default',
    name              VARCHAR(255) NOT NULL,
    description       TEXT NULL,
    agent_type        VARCHAR(32) NOT NULL,
    custom_agent_id   VARCHAR(255) NULL,
    cli_path          TEXT NULL,
    automation_config TEXT NOT NULL DEFAULT ('{}'),
    created_at        BIGINT NOT NULL,
    updated_at        BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_personal_agents_owner
    ON one_personal_agents (owner_user_id, updated_at DESC);
CREATE INDEX idx_one_personal_agents_tenant
    ON one_personal_agents (tenant_id);

CREATE TABLE IF NOT EXISTS one_employee_runs (
    id             VARCHAR(255) PRIMARY KEY,
    agent_id       VARCHAR(255) NOT NULL,
    owner_user_id  VARCHAR(255) NOT NULL,
    tenant_id      VARCHAR(255) NOT NULL DEFAULT 'default',
    team_id        VARCHAR(255) NULL,
    slot_id        VARCHAR(255) NULL,
    conversation_id VARCHAR(255) NOT NULL,
    turn_id        VARCHAR(255) NULL,
    status         VARCHAR(16) NOT NULL CHECK (status IN ('running', 'success', 'failed')),
    summary        TEXT NULL,
    error          TEXT NULL,
    trigger_source VARCHAR(16) NOT NULL DEFAULT 'manual',
    started_at     BIGINT NOT NULL,
    finished_at    BIGINT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_employee_runs_agent
    ON one_employee_runs (agent_id, started_at DESC);
CREATE INDEX idx_one_employee_runs_conversation
    ON one_employee_runs (conversation_id);
