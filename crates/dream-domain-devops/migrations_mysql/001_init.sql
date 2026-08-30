-- one-devops 001: requirements board + collaboration registries (MySQL port).
--
-- Rebuild of the 1ONE ClaudeCode DevOps slice consumed by the
-- superAssistant IssuesWorkbench + EnterpriseCollaboration panels.
-- Field vocabulary mirrors the 1one reference (RequirementType/Status/
-- Priority, SkillRecord, McpRegistryRecord, RagDocumentRecord). The v2
-- instance-per-enterprise model needs no tenant column.
--
-- MySQL notes: `type` is backticked (reserved word); `content` (skill
-- bodies) is LONGTEXT — MySQL TEXT caps at 64 KiB; conventions otherwise
-- per one-org migrations_mysql/001_init.sql.

CREATE TABLE IF NOT EXISTS one_requirements (
    id           VARCHAR(255) PRIMARY KEY NOT NULL,
    parent_id    VARCHAR(255) NULL,
    `type`       VARCHAR(16) NOT NULL DEFAULT 'task'
                 CHECK(`type` IN ('epic', 'feature', 'story', 'bug', 'task')),
    subject      VARCHAR(255) NOT NULL,
    description  LONGTEXT NULL,
    status       VARCHAR(16) NOT NULL DEFAULT 'backlog'
                 CHECK(status IN ('backlog', 'planning', 'developing', 'testing', 'completed')),
    priority     VARCHAR(16) NOT NULL DEFAULT 'medium'
                 CHECK(priority IN ('low', 'medium', 'high', 'urgent')),
    assigned_to  VARCHAR(255) NULL,
    milestone_id VARCHAR(255) NULL,
    creator_id   VARCHAR(255) NOT NULL,
    creator_name VARCHAR(255) NULL,
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_requirements_parent ON one_requirements (parent_id);
CREATE INDEX idx_one_requirements_status ON one_requirements (status, updated_at DESC);

CREATE TABLE IF NOT EXISTS one_requirement_comments (
    id             VARCHAR(255) PRIMARY KEY NOT NULL,
    requirement_id VARCHAR(255) NOT NULL,
    author_type    VARCHAR(16) NOT NULL DEFAULT 'user'
                   CHECK(author_type IN ('user', 'agent', 'autopilot')),
    author_id      VARCHAR(255) NULL,
    author_name    VARCHAR(255) NOT NULL,
    body           LONGTEXT NOT NULL,
    metadata       TEXT NULL,
    created_at     BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_requirement_comments_req
    ON one_requirement_comments (requirement_id, created_at);

CREATE TABLE IF NOT EXISTS one_skill_registry (
    id          VARCHAR(255) PRIMARY KEY NOT NULL,
    name        VARCHAR(255) NOT NULL,
    description TEXT NOT NULL DEFAULT (''),
    content     LONGTEXT NOT NULL DEFAULT (''),
    enabled     TINYINT(1) NOT NULL DEFAULT 1,
    scope       VARCHAR(16) NOT NULL DEFAULT 'org',
    team_id     VARCHAR(255) NULL,
    created_by  VARCHAR(255) NOT NULL,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_mcp_registry (
    id         VARCHAR(255) PRIMARY KEY NOT NULL,
    name       VARCHAR(255) NOT NULL,
    `type`     VARCHAR(16) NOT NULL DEFAULT 'stdio' CHECK(`type` IN ('stdio', 'sse')),
    endpoint   TEXT NOT NULL DEFAULT (''),
    enabled    TINYINT(1) NOT NULL DEFAULT 1,
    has_keys   TINYINT(1) NOT NULL DEFAULT 0,
    scope      VARCHAR(16) NOT NULL DEFAULT 'org',
    team_id    VARCHAR(255) NULL,
    created_by VARCHAR(255) NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_rag_documents (
    id          VARCHAR(255) PRIMARY KEY NOT NULL,
    title       VARCHAR(255) NOT NULL,
    file_path   TEXT NULL,
    file_size   BIGINT NULL,
    mime_type   VARCHAR(128) NULL,
    status      VARCHAR(16) NOT NULL DEFAULT 'pending',
    last_error  TEXT NULL,
    chunk_count BIGINT NOT NULL DEFAULT 0,
    scope       VARCHAR(16) NOT NULL DEFAULT 'org',
    team_id     VARCHAR(255) NULL,
    created_by  VARCHAR(255) NOT NULL,
    created_at  BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
