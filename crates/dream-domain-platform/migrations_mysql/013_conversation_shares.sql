-- MySQL twin of migrations/013_conversation_shares.sql (P2-2).
ALTER TABLE one_security_policy ADD COLUMN conversation_share_mode TEXT NOT NULL DEFAULT 'off';

CREATE TABLE IF NOT EXISTS one_conversation_shares (
    id              TEXT    PRIMARY KEY,
    conversation_id TEXT    NOT NULL UNIQUE,
    owner_user_id   TEXT    NOT NULL,
    enterprise_id   TEXT    NOT NULL,
    tenant_id       TEXT    NOT NULL,
    scope           TEXT    NOT NULL,
    name            TEXT    NOT NULL,
    uploaded        INTEGER NOT NULL DEFAULT 0,
    shared_at       INTEGER NOT NULL,
    INDEX idx_one_conv_shares_ent (enterprise_id, scope, shared_at DESC),
    INDEX idx_one_conv_shares_tenant (tenant_id, shared_at DESC),
    INDEX idx_one_conv_shares_owner (owner_user_id, shared_at DESC)
) ROW_FORMAT=DYNAMIC;
