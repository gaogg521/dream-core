-- Open integration API keys (E5 "开放集成"): the reference product's key
-- carries more than an identity — allowed path prefixes and a bound rate
-- limit. Same storage-before-enforcement posture as everything else added
-- alongside this migration: creating a key here does NOT let anything
-- authenticate with it yet. No auth-middleware path in this codebase
-- currently checks `one_api_keys` — see
-- `PlatformService::create_api_key`'s doc comment for the reasoning and
-- what wiring real enforcement would require (MySQL port).
--
-- The secret is generated once, shown to the admin exactly once, and never
-- stored in recoverable form — only its SHA-256 hash. `key_prefix` (the
-- first few characters of the plaintext secret) is kept so the admin console
-- can show "sk_live_ab12…" to tell keys apart without ever displaying the
-- full secret again, same "write-only" posture as a model channel's
-- credential (`ModelChannelsTab` on the frontend, `one_model_channels` on
-- the backend).
CREATE TABLE IF NOT EXISTS one_api_keys (
    id                    VARCHAR(255) PRIMARY KEY NOT NULL,
    tenant_id             VARCHAR(255) NOT NULL,
    name                  VARCHAR(255) NOT NULL,
    key_prefix            VARCHAR(255) NOT NULL,
    key_hash              VARCHAR(64) NOT NULL,   -- hex SHA-256
    -- JSON array of path prefixes this key may call once something enforces
    -- it, e.g. ["/api/one/devops/*"]. Empty = no path has been scoped in —
    -- deliberately NOT "every path", so a key created before an admin picks
    -- specific paths cannot be mistaken for an unrestricted one.
    allowed_paths         TEXT NOT NULL DEFAULT ('[]'),
    -- The "绑定策略" (bound policy) half: an optional rate-limit override for
    -- this specific key, independent of the tenant's own
    -- `one_security_policy.send_rate_limit_per_minute`. NULL = no per-key
    -- override.
    rate_limit_per_minute INT NULL,
    status                VARCHAR(16) NOT NULL DEFAULT 'active', -- 'active' | 'revoked'
    created_by            VARCHAR(255) NOT NULL,
    created_at            BIGINT NOT NULL,
    revoked_at            BIGINT NULL,
    -- Always NULL until something actually authenticates with these keys.
    last_used_at          BIGINT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_api_keys_tenant
    ON one_api_keys (tenant_id, status);
