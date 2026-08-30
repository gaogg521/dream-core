-- In-app notifications ("站内消息", align-openocta P2-3): broadcast /
-- targeted messages an admin composes in the console, plus per-recipient
-- read state for the home page's unread aggregation (MySQL port).
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
    id         VARCHAR(255) PRIMARY KEY NOT NULL,
    tenant_id  VARCHAR(255) NOT NULL,
    -- 'broadcast' | 'targeted'
    kind       VARCHAR(16) NOT NULL,
    category   VARCHAR(32) NOT NULL DEFAULT (''),
    title      VARCHAR(255) NOT NULL,
    body       TEXT NOT NULL,
    created_by VARCHAR(255) NOT NULL,
    created_at BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_notifications_tenant
    ON one_notifications (tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS one_notification_recipients (
    notification_id VARCHAR(255) NOT NULL,
    user_id         VARCHAR(255) NOT NULL,
    PRIMARY KEY (notification_id, user_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_notification_recipients_user
    ON one_notification_recipients (user_id);

CREATE TABLE IF NOT EXISTS one_notification_reads (
    notification_id VARCHAR(255) NOT NULL,
    user_id         VARCHAR(255) NOT NULL,
    read_at         BIGINT NOT NULL,
    PRIMARY KEY (notification_id, user_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
