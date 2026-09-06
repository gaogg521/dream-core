-- MySQL twin of migrations/billing_009_conversation_audit_requests.sql (P2-3).
CREATE TABLE IF NOT EXISTS one_conversation_audit_requests (
    id              TEXT    PRIMARY KEY,
    enterprise_id   TEXT    NOT NULL,
    tenant_id       TEXT    NOT NULL,
    target_user_id  TEXT    NOT NULL,
    conversation_id TEXT    NOT NULL,
    requested_by    TEXT    NOT NULL,
    requested_at    INTEGER NOT NULL,
    fulfilled_at    INTEGER
) ROW_FORMAT=DYNAMIC;
CREATE INDEX idx_one_conv_audit_req_target
    ON one_conversation_audit_requests(target_user_id, fulfilled_at);
CREATE INDEX idx_one_conv_audit_req_ent
    ON one_conversation_audit_requests(enterprise_id, requested_at DESC);
