-- In-app notifications ("站内消息", align-openocta P2-3): broadcast /
-- targeted messages an admin composes in the console, plus per-recipient
-- read state for the home page's unread aggregation.
--
-- Three tables, deliberately kept boring:
--
--   one_notifications             the message itself (tenant-scoped)
--   one_notification_recipients   explicit targets of a `targeted` message
--   one_notification_reads        who has read what, when
--
-- A `broadcast` row has NO recipient rows on purpose: its audience is
-- "every member of the tenant", evaluated at read time. That way members
-- who join after the send still see it — a fan-out snapshot taken at send
-- time would silently exclude them. A `targeted` row lists its recipients
-- explicitly and only those users (plus tenant admins, who read the sent
-- history through the admin list) ever see it.
--
-- `category` is a free-form short label the console shows as a tag (e.g.
-- 公告 / 安全 / 审批). Read state lives in its own table rather than a
-- column so both broadcast and targeted rows share one mechanism, and so
-- "mark read" never has to touch the message row.
CREATE TABLE IF NOT EXISTS one_notifications (
    id          TEXT    PRIMARY KEY NOT NULL,
    tenant_id   TEXT    NOT NULL,
    -- 'broadcast' | 'targeted'
    kind        TEXT    NOT NULL,
    category    TEXT    NOT NULL DEFAULT '',
    title       TEXT    NOT NULL,
    body        TEXT    NOT NULL,
    created_by  TEXT    NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_one_notifications_tenant
    ON one_notifications(tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS one_notification_recipients (
    notification_id TEXT    NOT NULL,
    user_id         TEXT    NOT NULL,
    PRIMARY KEY (notification_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_one_notification_recipients_user
    ON one_notification_recipients(user_id);

CREATE TABLE IF NOT EXISTS one_notification_reads (
    notification_id TEXT    NOT NULL,
    user_id         TEXT    NOT NULL,
    read_at         INTEGER NOT NULL,
    PRIMARY KEY (notification_id, user_id)
);
