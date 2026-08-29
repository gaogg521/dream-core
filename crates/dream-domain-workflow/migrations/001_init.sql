-- Approval workflow ("审批工作流", align-openocta P2-1): one table of
-- approval tasks, read through the looking glass three ways —
--
--   待办 (pending)     the admin queue: tasks awaiting a decision
--   已办 (decided)     approved / rejected / expired, with who and when
--   mine               what a member submitted and where it stands
--
-- `kind` discriminates the five OpenOcta-aligned approval classes
-- (creation / resource / security_policy_template / tool / prompt). The
-- workflow service is deliberately generic — kind-specific behaviour lives
-- with the submitter, encoded in `payload` JSON — so a sixth kind later is
-- a validated string, not a migration.
--
-- `expires_at` backs the terminal-tool approval flow (T8's
-- `terminal_tools_require_approval` field): the agent's hot path creates a
-- `tool` task and blocks until it is decided or the deadline passes —
-- **an expired task is a denial** (the conservative default; OpenOcta does
-- not expose its own value). Expired tasks are marked lazily by readers so
-- the decided view stays truthful without a scheduler.
CREATE TABLE IF NOT EXISTS one_workflow_tasks (
    id            TEXT    PRIMARY KEY NOT NULL,
    tenant_id     TEXT    NOT NULL,
    kind          TEXT    NOT NULL,
    title         TEXT    NOT NULL,
    detail        TEXT    NOT NULL DEFAULT '',
    payload       TEXT    NOT NULL DEFAULT '{}',
    requester_id  TEXT    NOT NULL,
    -- 'pending' | 'approved' | 'rejected' | 'expired'
    status        TEXT    NOT NULL DEFAULT 'pending',
    decided_by    TEXT,
    decided_at    INTEGER,
    note          TEXT,
    expires_at    INTEGER,
    created_at    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_one_workflow_tasks_tenant_status
    ON one_workflow_tasks(tenant_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_one_workflow_tasks_requester
    ON one_workflow_tasks(requester_id, created_at DESC);
