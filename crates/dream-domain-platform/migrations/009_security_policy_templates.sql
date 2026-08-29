-- Security policy templates (P1-8, align-openocta "安全策略模板层"): the
-- "策略模板" half of the reference product's two-layer model ("全局基线 /
-- 策略模板") that 005_security_policy.sql explicitly left out. Three layers,
-- with an honest boundary between them:
--
--   1. Template  (one_security_policy_templates)   a *named snapshot* of the
--      same seven policy fields the tenant baseline carries, plus provenance
--      (which built-in tier it was authored from — recorded, never consulted).
--   2. Bindings  (one_security_policy_bindings)    an *assignment ledger*: who
--      (a member or a department) this template has been allocated to. A
--      binding row does not change any behavior by itself — its only live
--      effect is the "覆盖实例数" the template list shows, which is simply
--      this table's row count per template.
--   3. Enforcement  stays on the tenant baseline (`one_security_policy`,
--      migration 005). `PlatformService::apply_policy_template` copies the
--      template's fields into the baseline as an explicit admin action.
--
-- Why bindings don't enforce per-subject: the real enforcement hot paths
-- (ACP tool-call permission routing, the send gate) read the one per-tenant
-- baseline row. Making a binding actually change a specific member's
-- behavior requires those paths to resolve a per-user policy — walking
-- subject_type/department ancestry on every tool call — which is its own
-- project, not a schema column. Until then a template is a named snapshot +
-- allocation record, and "apply" is the only thing that changes enforcement.
--
-- The field columns deliberately mirror 005's baseline exactly (same names,
-- same defaults, `send_rate_limit_per_minute` nullable) so a copy in either
-- direction is a column-for-column move with no translation step.

CREATE TABLE IF NOT EXISTS one_security_policy_templates (
    id                                  TEXT    PRIMARY KEY NOT NULL,
    tenant_id                           TEXT    NOT NULL,
    name                                TEXT    NOT NULL,
    description                         TEXT    NOT NULL DEFAULT '',
    -- Which built-in tier ('relaxed' | 'standard' | 'strict' | 'custom') the
    -- template's fields were authored from. Purely provenance for the UI —
    -- nothing reads it back for behavior.
    tier                                TEXT    NOT NULL DEFAULT 'custom',
    terminal_tools_require_approval     INTEGER NOT NULL,
    destructive_commands_blocked        INTEGER NOT NULL,
    -- JSON array, same shape as the baseline's `blocked_command_patterns`.
    blocked_command_patterns            TEXT    NOT NULL DEFAULT '[]',
    external_network_denied_by_default  INTEGER NOT NULL,
    message_scan_enabled                INTEGER NOT NULL,
    message_redact_enabled              INTEGER NOT NULL,
    -- NULL = unlimited, same as the baseline.
    send_rate_limit_per_minute          INTEGER,
    created_by                          TEXT    NOT NULL,
    created_at                          INTEGER NOT NULL,
    updated_at                          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_one_security_policy_templates_tenant
    ON one_security_policy_templates(tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS one_security_policy_bindings (
    id           TEXT    PRIMARY KEY NOT NULL,
    tenant_id    TEXT    NOT NULL,
    template_id  TEXT    NOT NULL,
    -- 'member' | 'department'. Members are validated against one_user_org at
    -- bind time (an admin told "bound" while the id was a typo would be a
    -- fake success, same posture as targeted notifications); departments are
    -- validated against one_departments the same way.
    subject_type TEXT    NOT NULL,
    subject_id   TEXT    NOT NULL,
    note         TEXT,
    bound_by     TEXT    NOT NULL,
    bound_at     INTEGER NOT NULL,
    UNIQUE(template_id, subject_type, subject_id)
);

CREATE INDEX IF NOT EXISTS idx_one_security_policy_bindings_subject
    ON one_security_policy_bindings(tenant_id, subject_type, subject_id);
