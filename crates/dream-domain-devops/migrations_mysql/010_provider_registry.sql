-- one-devops 010: company-provisioned model channels (T2) (MySQL port).
--
-- Until now the three registries distributed skills, MCP connectors and
-- knowledge — but never model credentials, so every employee had to obtain and
-- paste their own API key. For image and video models that is both expensive
-- and the thing enterprise procurement objects to hardest: keys handed out are
-- keys lost, and a leaver keeps theirs.
--
-- Unlike `one_mcp_registry` (008), the credential is NOT distributed. It stays
-- encrypted here and is only ever decrypted server-side by the model proxy.
-- Members receive a channel token instead, which is revocable and identifies
-- them at the proxy. That deliberately trades the offline-first property 008
-- chose for MCP: a company model channel needs the server to be reachable.
--
-- Scope / team_id / visibility mirror 009 exactly, so the existing
-- `member_visibility_where()` filter and `validate_resource_scope()` apply
-- unchanged: enterprise-wide by default, overridable per project group.

CREATE TABLE IF NOT EXISTS one_provider_registry (
    id                 VARCHAR(255) PRIMARY KEY NOT NULL,
    name               VARCHAR(255) NOT NULL,
    -- Platform id as understood by the desktop model catalogue (e.g. 'openai',
    -- 'dashscope'). Carried through to the materialized provider row so the
    -- client renders and routes it like any other.
    platform           VARCHAR(64) NOT NULL DEFAULT 'openai',
    -- Where the proxy forwards to. Stored exactly as an admin would type a
    -- provider base_url, because the proxy is path-preserving.
    upstream_base_url  TEXT NOT NULL,
    -- The real credential, encrypted with the deployment's data secret. Never
    -- returned by any endpoint and never distributed.
    api_key_encrypted  TEXT NOT NULL DEFAULT (''),
    -- JSON array of model names offered on this channel.
    models             TEXT NOT NULL DEFAULT ('[]'),
    -- JSON object of per-model settings (model_kind / media_endpoint /
    -- media_unit_price_usd), same shape as `providers.model_settings`.
    model_settings     TEXT NULL,
    enabled            TINYINT(1) NOT NULL DEFAULT 1,
    scope              VARCHAR(16) NOT NULL DEFAULT 'org',
    team_id            VARCHAR(255) NULL,
    visibility         VARCHAR(16) NOT NULL DEFAULT 'all',
    created_by         VARCHAR(255) NOT NULL,
    created_at         BIGINT NOT NULL,
    updated_at         BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_provider_registry_scope ON one_provider_registry (scope, team_id);

-- One long-lived, revocable token per (member, channel).
--
-- Deliberately not the session JWT: that rotates whenever membership changes
-- (one-org rotates it to kill a removed member's sessions), which would break
-- every provisioned channel until the next sync. A dedicated token is
-- decoupled from that, revocable per member on offboarding, and gives the
-- proxy per-member attribution — which is what a content audit will need.
--
-- Only the hash is stored: a leaked database must not yield working tokens.
CREATE TABLE IF NOT EXISTS one_provider_channel_tokens (
    token_hash VARCHAR(64) PRIMARY KEY NOT NULL,
    user_id    VARCHAR(255) NOT NULL,
    channel_id VARCHAR(255) NOT NULL,
    created_at BIGINT NOT NULL,
    last_used  BIGINT NULL,
    revoked_at BIGINT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_provider_channel_tokens_user ON one_provider_channel_tokens (user_id);
CREATE UNIQUE INDEX idx_one_provider_channel_tokens_pair
    ON one_provider_channel_tokens (user_id, channel_id);
