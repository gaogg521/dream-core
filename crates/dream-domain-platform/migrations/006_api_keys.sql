-- Open integration API keys (E5 "开放集成"): the reference product's key
-- carries more than an identity — allowed path prefixes and a bound rate
-- limit. Same storage-before-enforcement posture as everything else added
-- alongside this migration: creating a key here does NOT let anything
-- authenticate with it yet. No auth-middleware path in this codebase
-- currently checks `one_api_keys` — see
-- `PlatformService::create_api_key`'s doc comment for the reasoning and
-- what wiring real enforcement would require.
--
-- The secret is generated once, shown to the admin exactly once, and never
-- stored in recoverable form — only its SHA-256 hash. `key_prefix` (the
-- first few characters of the plaintext secret) is kept so the admin console
-- can show "sk_live_ab12…" to tell keys apart without ever displaying the
-- full secret again, same "write-only" posture as a model channel's
-- credential (`ModelChannelsTab` on the frontend, `one_model_channels` on
-- the backend).
CREATE TABLE IF NOT EXISTS one_api_keys (
    id                     TEXT    PRIMARY KEY NOT NULL,
    tenant_id              TEXT    NOT NULL,
    name                   TEXT    NOT NULL,
    key_prefix             TEXT    NOT NULL,
    key_hash               TEXT    NOT NULL,
    -- JSON array of path prefixes this key may call once something enforces
    -- it, e.g. ["/api/one/devops/*"]. Empty = no path has been scoped in —
    -- deliberately NOT "every path", so a key created before an admin picks
    -- specific paths cannot be mistaken for an unrestricted one.
    allowed_paths          TEXT    NOT NULL DEFAULT '[]',
    -- The "绑定策略" (bound policy) half: an optional rate-limit override for
    -- this specific key, independent of the tenant's own
    -- `one_security_policy.send_rate_limit_per_minute`. NULL = no per-key
    -- override.
    rate_limit_per_minute  INTEGER,
    status                 TEXT    NOT NULL DEFAULT 'active', -- 'active' | 'revoked'
    created_by             TEXT    NOT NULL,
    created_at             INTEGER NOT NULL,
    revoked_at             INTEGER,
    -- Always NULL until something actually authenticates with these keys.
    last_used_at           INTEGER
);

CREATE INDEX IF NOT EXISTS idx_one_api_keys_tenant
    ON one_api_keys(tenant_id, status);
