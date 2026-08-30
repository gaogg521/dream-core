-- Remote content market (P1-1 round 2, align-openocta §4): admin-curated
-- sources — an index.json manifest at an HTTP(S) URL (a bare static file or
-- a git repository served through its raw endpoints both qualify) — synced
-- into the skill/MCP registries with origin='market' (MySQL port).
--
-- Trust model, in one place: NOTHING ships pre-trusted. A source exists
-- because an administrator typed its URL in, which is exactly the same act
-- and the same trust level as uploading a SKILL.md by hand. Import-time
-- validation reuses the same frontmatter parser uploads go through; there
-- is deliberately no import-time sandbox — imported content is DATA, and
-- the runtime already enforces the full policy stack (send gate, terminal
-- tool security gate, content inspection) on whatever agents later do with
-- it. HTTP (not just HTTPS) is accepted for intranet sources; the cleartext
-- caveat is the admin's to weigh, same as every other endpoint URL this
-- console stores.
--
-- Incremental semantics: per-item SHA-256 against one_market_imports.
-- Same hash → skipped (no fetch, no write); changed or new → fetched and
-- upserted; an item that vanished from the upstream index is REPORTED and
-- its local row KEPT — published content must not disappear because the
-- upstream shuffled its manifest; the admin deletes it explicitly if they
-- agree. Re-sync is idempotent.
--
-- Registry linkage lives HERE (one_market_imports), not as columns on the
-- registries: the registries keep no knowledge of where market rows came
-- from, and a name collision with a row this source did not import is an
-- error, never a takeover (same rule the P1-6 publish path enforces).
CREATE TABLE IF NOT EXISTS one_market_sources (
    id               VARCHAR(255) PRIMARY KEY,
    tenant_id        VARCHAR(255) NOT NULL,
    name             VARCHAR(191) NOT NULL,
    url              TEXT NOT NULL,
    enabled          TINYINT(1) NOT NULL DEFAULT 1,
    last_synced_at   BIGINT NULL,
    last_sync_status VARCHAR(16) NULL,  -- 'ok' | 'error' | NULL (never synced)
    last_sync_error  TEXT NULL,
    created_by       VARCHAR(255) NOT NULL,
    created_at       BIGINT NOT NULL,
    updated_at       BIGINT NOT NULL,
    UNIQUE KEY uq_one_market_sources_tenant_name (tenant_id, name)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_market_imports (
    source_id    VARCHAR(255) NOT NULL,
    kind         VARCHAR(16) NOT NULL, -- 'skill' | 'mcp'
    item_name    VARCHAR(255) NOT NULL, -- manifest item name (the mapping key)
    registry_id  VARCHAR(255) NOT NULL,
    content_hash VARCHAR(64) NOT NULL,
    updated_at   BIGINT NOT NULL,
    PRIMARY KEY (source_id, kind, item_name)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
