-- 058: 可撤销的 WebUI 设备会话与审计记录。
--
-- 设备 ID 会被写入扫码/密码登录签发的 JWT。服务端保留令牌摘要而非原文，
-- 因此可单独撤销一台手机，不需要让同一账号的所有桌面会话失效。

CREATE TABLE IF NOT EXISTS webui_device_sessions (
    id            TEXT PRIMARY KEY NOT NULL,
    user_id       TEXT NOT NULL,
    label         TEXT NOT NULL,
    token_hash    TEXT NOT NULL UNIQUE,
    paired_at     INTEGER NOT NULL,
    last_seen_at  INTEGER NOT NULL,
    last_ip       TEXT,
    revoked_at    INTEGER
);

CREATE INDEX IF NOT EXISTS idx_webui_device_sessions_user_active
    ON webui_device_sessions(user_id, revoked_at, last_seen_at DESC);

CREATE TABLE IF NOT EXISTS webui_device_audit (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    user_id    TEXT NOT NULL,
    device_id  TEXT,
    action     TEXT NOT NULL,
    detail     TEXT,
    ip         TEXT
);

CREATE INDEX IF NOT EXISTS idx_webui_device_audit_user_ts
    ON webui_device_audit(user_id, ts DESC);
