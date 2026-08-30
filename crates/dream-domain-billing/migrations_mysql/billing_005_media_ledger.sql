-- T8: a consolidated, searchable ledger of every generated media asset —
-- distinct from `one_usage_events`, which records cost/attribution only and
-- deliberately carries neither a file path nor a prompt (see `MediaUsage`)
-- (MySQL port).
--
-- One row per generated FILE (not per job — a job producing 4 images writes 4
-- rows), so each artifact is individually findable. Enterprise-scoped only:
-- personal/no-company users are never recorded here, same red line every
-- other governance surface in this crate honors.
CREATE TABLE IF NOT EXISTS one_media_assets (
    id              VARCHAR(255) PRIMARY KEY,
    user_id         VARCHAR(255) NOT NULL,
    enterprise_id   VARCHAR(255) NOT NULL,
    department_id   VARCHAR(255) NULL,
    conversation_id VARCHAR(255) NULL,
    kind            VARCHAR(32) NOT NULL,
    model           VARCHAR(255) NULL,
    file_path       TEXT NOT NULL,
    -- NULL unless the company has opted into prompt retention
    -- (`one_media_ledger_settings.retain_prompts`). The server enforces this,
    -- never the caller: a client always reports the true prompt, and this
    -- column silently stays NULL when the company has not opted in.
    -- LONGTEXT: user-controlled free text; MySQL TEXT caps at 64 KiB.
    prompt          LONGTEXT NULL,
    created_at      BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_media_assets_enterprise ON one_media_assets (enterprise_id, created_at DESC);
CREATE INDEX idx_one_media_assets_user ON one_media_assets (user_id, created_at DESC);
CREATE INDEX idx_one_media_assets_conversation ON one_media_assets (conversation_id);

-- One row per company that has ever touched the setting. Absence = the
-- default (prompts not retained) — same "no row = ungoverned/default" idiom
-- as `one_department_budgets`.
CREATE TABLE IF NOT EXISTS one_media_ledger_settings (
    enterprise_id  VARCHAR(255) PRIMARY KEY,
    retain_prompts TINYINT(1) NOT NULL DEFAULT 0,
    updated_at     BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
