-- P2-2: member-to-organization conversation sharing, gated by the
-- per-tenant `conversation_share_mode` policy column added here.
--
-- `one_security_policy.conversation_share_mode`:
--   'off'        (default) nothing is shared or uploaded; the share
--                endpoints refuse. WebUI-native sharing of server-side
--                conversations is unaffected only in the sense that this
--                whole subsystem is off — every share goes through it.
--   'tenant'     members may share into their own project group (tenant).
--   'enterprise' members may share company-wide as well.
-- Unlike the other columns of this table, this one HAS an enforcer (the
-- share endpoints below); the tier presets still reset it to 'off' so a
-- tier change always re-opens sharing as a deliberate act.
--
-- `one_conversation_shares` is the authorization row: one active share per
-- conversation (UNIQUE), pointing at a conversation in the main schema.
-- For a member in client mode the referenced row is a SNAPSHOT the client
-- uploaded at share time (created through the conversation repository under
-- the member's own user id, marked in `conversations.extra`); for a WebUI
-- member it is their existing conversation. Reads authorize through this
-- table only — there is no other cross-user read path.
ALTER TABLE one_security_policy ADD COLUMN conversation_share_mode TEXT NOT NULL DEFAULT 'off';

CREATE TABLE IF NOT EXISTS one_conversation_shares (
    id              TEXT    PRIMARY KEY,
    conversation_id TEXT    NOT NULL UNIQUE,
    owner_user_id   TEXT    NOT NULL,
    enterprise_id   TEXT    NOT NULL,
    tenant_id       TEXT    NOT NULL,
    -- 'tenant' | 'enterprise'
    scope           TEXT    NOT NULL,
    -- Denormalized conversation title, so inbox listings need no join into
    -- the main schema (which lives on a different backend under MySQL).
    name            TEXT    NOT NULL,
    -- 1 when the share points at a snapshot the owner's client uploaded at
    -- share time (client-mode desktop conversations), 0 for a native
    -- server-side conversation.
    uploaded        INTEGER NOT NULL DEFAULT 0,
    shared_at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_one_conv_shares_ent
    ON one_conversation_shares(enterprise_id, scope, shared_at DESC);
CREATE INDEX IF NOT EXISTS idx_one_conv_shares_tenant
    ON one_conversation_shares(tenant_id, shared_at DESC);
CREATE INDEX IF NOT EXISTS idx_one_conv_shares_owner
    ON one_conversation_shares(owner_user_id, shared_at DESC);
