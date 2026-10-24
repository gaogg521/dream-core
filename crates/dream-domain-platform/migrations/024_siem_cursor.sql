-- How far the audit log has been forwarded to the company's SIEM. The forwarder
-- sends events strictly after (last_created_at, last_id) and only moves this
-- forward once the receiver accepted the batch, so a down SIEM loses nothing and
-- a restart never re-sends or skips.
CREATE TABLE IF NOT EXISTS one_siem_cursor (
    tenant_id       TEXT    PRIMARY KEY NOT NULL,
    last_created_at INTEGER NOT NULL,
    last_id         TEXT    NOT NULL DEFAULT '',
    forwarded_total INTEGER NOT NULL DEFAULT 0,
    last_run_at     INTEGER,
    last_status     TEXT,
    last_error      TEXT
);
