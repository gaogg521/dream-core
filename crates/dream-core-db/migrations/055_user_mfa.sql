-- 055: 登录二次认证（MFA · TOTP）
--
-- users 表新增：
--   mfa_secret_cipher  AES-256-GCM 密文（key 由该用户 data_secret 派生），
--                      NULL = 未绑定。库里永远没有可用于伪造动态码的明文。
--   mfa_enabled        绑定是否已生效（必须先成功验证一次动态码才置 1）
--   mfa_bound_at       绑定完成时间（epoch ms）
--   mfa_exempt         豁免：管理员标记为无需第二步（服务号/API/定时任务账号）
--   mfa_force          单用户强制绑定：全局档为「可选」时该用户仍被阻断绑定
--   mfa_last_step      最近一次通过验证的时间片（防同一动态码重放）
--
-- mfa_policy：全局档位（单行）。mode = off | optional | mandatory。
--   缺省（无行）按 off 处理——存量部署升级后登录行为不变，由管理员显式开启。
--
-- mfa_challenges：第一步通过后签发的一次性临时凭证（≤5 分钟，只存 sha256 哈希，
--   不存原文）。attempts 记录失败次数（上限 5 次，超限作废）；
--   pending_secret_cipher 仅为「强制绑定」挑战暂存待确认的密钥密文。
--
-- mfa_audit：MFA 相关动作留痕（时间/账号/动作/详情/来源 IP）。

ALTER TABLE users ADD COLUMN mfa_secret_cipher TEXT;
ALTER TABLE users ADD COLUMN mfa_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN mfa_bound_at INTEGER;
ALTER TABLE users ADD COLUMN mfa_exempt INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN mfa_force INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN mfa_last_step INTEGER;

CREATE TABLE IF NOT EXISTS mfa_policy (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    mode       TEXT NOT NULL DEFAULT 'off',
    updated_by TEXT,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS mfa_challenges (
    token_hash            TEXT PRIMARY KEY NOT NULL,
    user_id               TEXT NOT NULL,
    purpose               TEXT NOT NULL,
    attempts              INTEGER NOT NULL DEFAULT 0,
    expires_at            INTEGER NOT NULL,
    used                  INTEGER NOT NULL DEFAULT 0,
    pending_secret_cipher TEXT,
    redirect_target       TEXT,
    desktop               INTEGER NOT NULL DEFAULT 0,
    scheme                TEXT,
    created_ip            TEXT,
    created_at            INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_mfa_challenges_user ON mfa_challenges(user_id);

CREATE TABLE IF NOT EXISTS mfa_audit (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    ts       INTEGER NOT NULL,
    user_id  TEXT,
    username TEXT,
    action   TEXT NOT NULL,
    detail   TEXT,
    ip       TEXT
);

CREATE INDEX IF NOT EXISTS idx_mfa_audit_ts ON mfa_audit(ts DESC);
