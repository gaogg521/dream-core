-- one-billing: the commercialization "billing plane" — subscription tier,
-- seat cap, and per-turn usage metering. Deliberately separate from
-- one-enterprise (identity) so licensing/usage can evolve independently
-- (MySQL port).
--
-- License attaches to an SSO company (one-enterprise's `one_enterprises`).
-- Personal / standalone users have no enterprise and are NOT in this system.
--
-- MySQL port conventions: BIGINT timestamps, utf8mb4_0900_as_cs — see
-- one-org migrations_mysql/001_init.sql.

CREATE TABLE IF NOT EXISTS one_enterprise_license (
    enterprise_id VARCHAR(255) PRIMARY KEY,
    -- 'free' | 'team' | 'enterprise'. New companies default 'free' (set on
    -- create/sync); existing companies are grandfathered below.
    tier          VARCHAR(16) NOT NULL DEFAULT 'free',
    -- Explicit seat override; NULL = use the tier's default cap.
    seat_limit    BIGINT NULL,
    -- License expiry (ms); NULL = no expiry. Informational for now.
    expires_at    BIGINT NULL,
    updated_at    BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

-- One row per metered turn. `enterprise_id` NULL for personal users (kept
-- locally but never shown in a company dashboard). Token columns are
-- best-effort (sourced from ACP context usage); NULL when unavailable.
CREATE TABLE IF NOT EXISTS one_usage_events (
    id                     VARCHAR(255) PRIMARY KEY,
    user_id                VARCHAR(255) NOT NULL,
    enterprise_id          VARCHAR(255) NULL,
    conversation_id        VARCHAR(255) NULL,
    model                  VARCHAR(255) NULL,
    input_tokens           BIGINT NULL,
    output_tokens          BIGINT NULL,
    total_tokens           BIGINT NULL,
    estimated_cost_micros  BIGINT NULL,
    created_at             BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_usage_events_ent  ON one_usage_events (enterprise_id, created_at DESC);
CREATE INDEX idx_one_usage_events_user ON one_usage_events (user_id, created_at DESC);

-- Grandfather every company that already exists to the top tier, so a
-- pre-billing enterprise keeps all the features it was already using. New
-- companies created after this migration default to 'free'. Runs after the
-- one-enterprise migrations (ordering enforced in dream-core-app).
INSERT IGNORE INTO one_enterprise_license (enterprise_id, tier, updated_at)
    SELECT id, 'enterprise', CAST(UNIX_TIMESTAMP() AS UNSIGNED) * 1000
    FROM one_enterprises;
