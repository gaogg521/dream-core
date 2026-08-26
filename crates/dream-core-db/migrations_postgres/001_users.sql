-- Postgres port of the `users` table, as it exists today after SQLite
-- migrations 001 (initial columns), 025 (data_secret), 042 (user_type /
-- external_user_id / status / session_generation, table rebuild), and 046
-- (adopted_by / adopted_at). Verified by reading all four files end to end —
-- no other file in migrations/ touches `users`.
--
-- This is a fresh-install schema, not a replayed migration history: a new
-- Postgres deployment has no legacy SQLite data to normalize, so none of the
-- intermediate `users_new` rebuild steps or the other 48 SQLite migrations
-- (which are almost entirely personal-edition data fixes irrelevant to a
-- fresh enterprise install) are ported. Only the tables the enterprise admin
-- backend actually reads/writes get ported here, table by table, as that
-- work proceeds — see docs/e-pg-postgres-support.md for what's covered.
--
-- Dialect notes:
-- - SQLite's dynamically-typed INTEGER stores epoch-millisecond timestamps,
--   which exceed Postgres's 32-bit INTEGER range (max ~2.1e9). Every
--   timestamp column here is BIGINT instead.
-- - CHECK constraints and partial unique indexes (`WHERE ... IS NOT NULL`)
--   are supported by Postgres with identical syntax to SQLite.

CREATE TABLE IF NOT EXISTS users (
    id                 TEXT PRIMARY KEY NOT NULL,
    user_type          TEXT NOT NULL DEFAULT 'local'
                           CHECK (user_type IN ('local', 'aionpro')),
    external_user_id   TEXT,
    username           TEXT,
    email              TEXT,
    password_hash      TEXT,
    avatar_path        TEXT,
    jwt_secret         TEXT,
    data_secret        TEXT,
    status             TEXT NOT NULL DEFAULT 'active'
                           CHECK (status IN ('active', 'disabled')),
    -- BIGINT, not INTEGER: every Rust layer models this as `i64`
    -- (`models/user.rs`, `dream-core-auth::jwt`, `dream-core-api-types::auth`),
    -- and sqlx's Postgres decode is an exact type match — `i64` against an
    -- INT4 column fails at read time with a ColumnDecode error even though
    -- the value always fits.
    session_generation BIGINT NOT NULL DEFAULT 0,
    created_at         BIGINT NOT NULL,
    updated_at         BIGINT NOT NULL,
    last_login         BIGINT,
    adopted_by         TEXT,
    adopted_at         BIGINT,
    CHECK (
        (user_type = 'local' AND password_hash IS NOT NULL)
        OR
        (user_type = 'aionpro')
    ),
    CHECK (
        (external_user_id IS NULL)
        OR
        (length(external_user_id) > 0)
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_local_username
    ON users (username)
    WHERE user_type = 'local' AND username IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email
    ON users (email)
    WHERE email IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_external_user
    ON users (user_type, external_user_id)
    WHERE external_user_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_users_username ON users (username);
CREATE INDEX IF NOT EXISTS idx_users_status ON users (status);
