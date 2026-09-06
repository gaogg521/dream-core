-- P2-3: admin-initiated conversation upload requests (the on_demand tier
-- of the P0-1 decision). An admin may request the content of a member
-- conversation the server does not hold (client-mode desktop conversations
-- live on the member's machine); the member's client picks the request up on
-- its next poll and fulfils it by uploading a snapshot. Every admin READ of
-- conversation content is audited separately (see the service impl) — the
-- request itself is also an audited event.
CREATE TABLE IF NOT EXISTS one_conversation_audit_requests (
    id              TEXT    PRIMARY KEY,
    enterprise_id   TEXT    NOT NULL,
    tenant_id       TEXT    NOT NULL,
    target_user_id  TEXT    NOT NULL,
    -- The conversation id the admin asked for. For a client-mode member this
    -- is the LOCAL id; the uploaded snapshot reuses it (different database),
    -- so the admin reads by the same id either way.
    conversation_id TEXT    NOT NULL,
    requested_by    TEXT    NOT NULL,
    requested_at    INTEGER NOT NULL,
    fulfilled_at    INTEGER
);
CREATE INDEX IF NOT EXISTS idx_one_conv_audit_req_target
    ON one_conversation_audit_requests(target_user_id, fulfilled_at);
CREATE INDEX IF NOT EXISTS idx_one_conv_audit_req_ent
    ON one_conversation_audit_requests(enterprise_id, requested_at DESC);
