-- MySQL port of the `users` table, as it exists today after SQLite
-- migrations 001 (initial columns), 025 (data_secret), 042 (user_type /
-- external_user_id / status / session_generation, table rebuild), and 046
-- (adopted_by / adopted_at) — the same final state as
-- migrations_postgres/001_users.sql, re-expressed for MySQL 8.0.16+.
--
-- This is a fresh-install schema, not a replayed migration history: a MySQL
-- enterprise deployment has no legacy SQLite data to normalize. The main
-- conversation schema (`messages`, `conversations`, …) stays on SQLite —
-- mixed storage by design (P3-3 implementation plan §4); only the tables the
-- enterprise domain crates read/write from the MySQL side get ported here.
--
-- Dialect notes (MySQL vs the SQLite original):
-- - SQLite's dynamically-typed INTEGER stores epoch-millisecond timestamps,
--   which exceed MySQL's 32-bit INTEGER range. Every timestamp column is
--   BIGINT.
-- - Every table sets COLLATE utf8mb4_0900_as_cs (case-sensitive). The server
--   default `_ai_ci` would make `WHERE username = 'API'` match 'api' and
--   change auth/lookup semantics.
-- - SQLite's partial unique indexes have no MySQL equivalent. `email` and
--   `(user_type, external_user_id)` map to plain unique indexes: MySQL unique
--   indexes admit multiple NULLs, which is exactly what the SQLite partial
--   predicates (`WHERE ... IS NOT NULL`) carve out. `username` is only unique
--   among local users, so uniqueness rides on a stored generated column that
--   is NULL for every non-local row.
-- - `CHECK` constraints are enforced from MySQL 8.0.16, the minimum target.

CREATE TABLE IF NOT EXISTS users (
    id                 VARCHAR(255) PRIMARY KEY NOT NULL,
    user_type          VARCHAR(32) NOT NULL DEFAULT 'local'
                           CHECK (user_type IN ('local', 'aionpro')),
    external_user_id   VARCHAR(255) NULL,
    username           VARCHAR(255) NULL,
    email              VARCHAR(255) NULL,
    password_hash      TEXT NULL,
    avatar_path        TEXT NULL,
    jwt_secret         TEXT NULL,
    data_secret        TEXT NULL,
    status             VARCHAR(16) NOT NULL DEFAULT 'active'
                           CHECK (status IN ('active', 'disabled')),
    session_generation BIGINT NOT NULL DEFAULT 0,
    -- MFA (TOTP) binding + policy flags (SQLite migration 055 parity)
    mfa_secret_cipher  TEXT NULL,
    mfa_enabled        INTEGER NOT NULL DEFAULT 0,
    mfa_bound_at       BIGINT NULL,
    mfa_exempt         INTEGER NOT NULL DEFAULT 0,
    mfa_force          INTEGER NOT NULL DEFAULT 0,
    mfa_last_step      BIGINT NULL,
    created_at         BIGINT NOT NULL,
    updated_at         BIGINT NOT NULL,
    last_login         BIGINT NULL,
    adopted_by         VARCHAR(255) NULL,
    adopted_at         BIGINT NULL,
    -- NULL for every non-local row, so unique-index membership matches the
    -- SQLite partial index `WHERE user_type = 'local' AND username IS NOT NULL`.
    local_username     VARCHAR(255) AS (
                           CASE WHEN user_type = 'local' THEN username END
                       ) STORED,
    CHECK (
        (user_type = 'local' AND password_hash IS NOT NULL)
        OR
        (user_type = 'aionpro')
    ),
    CHECK (
        (external_user_id IS NULL)
        OR
        (CHAR_LENGTH(external_user_id) > 0)
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE UNIQUE INDEX idx_users_local_username ON users (local_username);
CREATE UNIQUE INDEX idx_users_email ON users (email);
CREATE UNIQUE INDEX idx_users_external_user ON users (user_type, external_user_id);
CREATE INDEX idx_users_username ON users (username);
CREATE INDEX idx_users_status ON users (status);
