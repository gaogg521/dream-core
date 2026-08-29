-- Enterprise memory ("记忆系统", align-openocta P2-2): three collection
-- tiers, soft refinement, and explicit grants.
--
-- `one_memory_collections.scope` selects the access model:
--   global     company-wide knowledge; every tenant member reads it by
--              default, writing needs an admin or a `write` grant
--   department fused per-department memory; bound to `department_id`,
--              readable by that department's members, writing needs a grant
--   personal   one member's distillation + preference learning; bound to
--              `owner_user_id`, owner-only — the owner never needs a grant
-- The tier invariants (global carries neither department_id nor
-- owner_user_id; department must carry department_id; personal must carry
-- owner_user_id) are enforced by the service at insert time, so a malformed
-- tier can never be stored even though SQLite would accept it.
--
-- `one_memory_items.content_hash` is the hex SHA-256 of `content`, computed
-- once at insert: the refine job groups duplicates by it instead of re-
-- hashing on every run. Refinement is deliberately soft — duplicates merged
-- away and low-value trims flip `status` to 'trimmed' but keep the row, so
-- history stays auditable; only search hides trimmed items.
--
-- `one_memory_grants` is the delegation table: read or write per subject
-- (a member, or a whole department) per collection. Global is readable
-- tenant-wide by default, so grants exist to open department/personal
-- collections and to open writes — the coverage metric derived from it
-- measures how much of the tenant can actually reach at least one active
-- memory.
CREATE TABLE IF NOT EXISTS one_memory_collections (
    id            TEXT    PRIMARY KEY NOT NULL,
    tenant_id     TEXT    NOT NULL,
    -- 'global' | 'department' | 'personal'
    scope         TEXT    NOT NULL,
    department_id TEXT,
    owner_user_id TEXT,
    name          TEXT    NOT NULL,
    description   TEXT    NOT NULL DEFAULT '',
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_one_memory_collections_tenant_scope
    ON one_memory_collections(tenant_id, scope);

CREATE TABLE IF NOT EXISTS one_memory_items (
    id                    TEXT    PRIMARY KEY NOT NULL,
    tenant_id             TEXT    NOT NULL,
    collection_id         TEXT    NOT NULL,
    content               TEXT    NOT NULL,
    content_hash          TEXT    NOT NULL,
    importance            REAL    NOT NULL DEFAULT 0.5,
    source_conversation_id TEXT,
    tags                  TEXT    NOT NULL DEFAULT '[]',
    -- 'active' | 'trimmed'
    status                TEXT    NOT NULL DEFAULT 'active',
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_one_memory_items_collection_status
    ON one_memory_items(collection_id, status, created_at);

CREATE TABLE IF NOT EXISTS one_memory_refine_jobs (
    id            TEXT    PRIMARY KEY NOT NULL,
    tenant_id     TEXT    NOT NULL,
    collection_id TEXT    NOT NULL,
    -- 'done' | 'failed'
    status        TEXT    NOT NULL DEFAULT 'done',
    merged_count  INTEGER NOT NULL DEFAULT 0,
    trimmed_count INTEGER NOT NULL DEFAULT 0,
    error         TEXT,
    created_at    INTEGER NOT NULL,
    finished_at   INTEGER
);

CREATE TABLE IF NOT EXISTS one_memory_grants (
    id            TEXT    PRIMARY KEY NOT NULL,
    tenant_id     TEXT    NOT NULL,
    collection_id TEXT    NOT NULL,
    -- 'member' | 'department'
    subject_type  TEXT    NOT NULL,
    subject_id    TEXT    NOT NULL,
    -- 'read' | 'write'
    access        TEXT    NOT NULL,
    granted_by    TEXT    NOT NULL,
    created_at    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_one_memory_grants_tenant_collection
    ON one_memory_grants(tenant_id, collection_id);
