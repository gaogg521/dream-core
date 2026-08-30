-- one-platform reserved infrastructure config (P1-3 containerization + P2-2
-- realtime collaboration). Both are per-project-group singletons keyed by
-- tenant_id, admin-managed, with any credential encrypted at rest (MySQL
-- port; conventions: BIGINT timestamps, utf8mb4_0900_as_cs, TINYINT(1)
-- flags — see one-org migrations_mysql/001_init.sql).
--
-- "Reserved adapter" pattern (mirrors one_smtp_config / one_integrations):
-- storing a row does NOT run anything — no container runtime or collaboration
-- backend is wired in. It lets an admin fill in config ahead of time so a real
-- `ContainerRuntime` / `CollaborationProvider` can be dropped in at the app
-- layer later without a schema change. `enabled` defaults off so a half-filled
-- config stays inert.

-- P1-3: where a project group's agents/workspaces run (Docker / K8s / ...).
CREATE TABLE IF NOT EXISTS one_container_config (
    tenant_id                 VARCHAR(255) PRIMARY KEY NOT NULL,
    runtime_kind              VARCHAR(32) NULL,          -- 'docker' | 'kubernetes' | 'none'
    endpoint                  TEXT NULL,                 -- docker socket / k8s api server
    default_image             VARCHAR(255) NULL,         -- image agents run in
    registry                  VARCHAR(255) NULL,         -- image registry host
    registry_secret_encrypted TEXT NULL,                 -- registry pull credential (encrypted)
    enabled                   TINYINT(1) NOT NULL DEFAULT 0,
    updated_at                BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

-- P2-2: the realtime collaboration backend (presence + shared sessions).
CREATE TABLE IF NOT EXISTS one_collaboration_config (
    tenant_id        VARCHAR(255) PRIMARY KEY NOT NULL,
    provider         VARCHAR(32) NULL,  -- 'builtin' | 'external' | 'none'
    endpoint         TEXT NULL,         -- external relay / CRDT sync endpoint
    secret_encrypted TEXT NULL,         -- auth token for the relay (encrypted)
    presence         TINYINT(1) NOT NULL DEFAULT 0,  -- broadcast presence/cursors
    enabled          TINYINT(1) NOT NULL DEFAULT 0,
    updated_at       BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
