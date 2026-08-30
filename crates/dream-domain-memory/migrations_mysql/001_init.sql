-- Enterprise memory ("记忆系统", align-openocta P2-2): three collection
-- tiers, soft refinement, and explicit grants (MySQL port).
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
--
-- MySQL port: content is user-controlled free text → LONGTEXT (MySQL TEXT
-- caps at 64 KiB); REAL → DOUBLE.
CREATE TABLE IF NOT EXISTS one_memory_collections (
    id            VARCHAR(255) PRIMARY KEY NOT NULL,
    tenant_id     VARCHAR(255) NOT NULL,
    -- 'global' | 'department' | 'personal'
    scope         VARCHAR(16) NOT NULL,
    department_id VARCHAR(255) NULL,
    owner_user_id VARCHAR(255) NULL,
    name          VARCHAR(255) NOT NULL,
    description   TEXT NOT NULL DEFAULT (''),
    created_at    BIGINT NOT NULL,
    updated_at    BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_memory_collections_tenant_scope
    ON one_memory_collections (tenant_id, scope);

CREATE TABLE IF NOT EXISTS one_memory_items (
    id                     VARCHAR(255) PRIMARY KEY NOT NULL,
    tenant_id              VARCHAR(255) NOT NULL,
    collection_id          VARCHAR(255) NOT NULL,
    content                LONGTEXT NOT NULL,
    content_hash           VARCHAR(64) NOT NULL,
    importance             DOUBLE NOT NULL DEFAULT 0.5,
    source_conversation_id VARCHAR(255) NULL,
    tags                   TEXT NOT NULL DEFAULT ('[]'),
    -- 'active' | 'trimmed'
    status                 VARCHAR(16) NOT NULL DEFAULT 'active',
    created_at             BIGINT NOT NULL,
    updated_at             BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_memory_items_collection_status
    ON one_memory_items (collection_id, status, created_at);

CREATE TABLE IF NOT EXISTS one_memory_refine_jobs (
    id            VARCHAR(255) PRIMARY KEY NOT NULL,
    tenant_id     VARCHAR(255) NOT NULL,
    collection_id VARCHAR(255) NOT NULL,
    -- 'done' | 'failed'
    status        VARCHAR(16) NOT NULL DEFAULT 'done',
    merged_count  BIGINT NOT NULL DEFAULT 0,
    trimmed_count BIGINT NOT NULL DEFAULT 0,
    error         TEXT NULL,
    created_at    BIGINT NOT NULL,
    finished_at   BIGINT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_memory_grants (
    id            VARCHAR(255) PRIMARY KEY NOT NULL,
    tenant_id     VARCHAR(255) NOT NULL,
    collection_id VARCHAR(255) NOT NULL,
    -- 'member' | 'department'
    subject_type  VARCHAR(16) NOT NULL,
    subject_id    VARCHAR(255) NOT NULL,
    -- 'read' | 'write'
    access        VARCHAR(16) NOT NULL,
    granted_by    VARCHAR(255) NOT NULL,
    created_at    BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_memory_grants_tenant_collection
    ON one_memory_grants (tenant_id, collection_id);
