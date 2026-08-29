-- P1-1 round 1 (align-openocta §4): shared category tree + tag tables for the
-- three content-registry resource types (skill / mcp / employee). One shared
-- table set filtered by `resource_type` rather than three separate schemas —
-- unlike the employee-grants split (migration 006), all three resource
-- shapes here are identical ("attach a category + zero or more tags to an
-- id"), so there's no semantic conflict that would justify separate tables.
--
-- Lives in dream-domain-employee (not dream-domain-devops, and not
-- dream-domain-platform) because dream-domain-devops already has a real
-- Cargo dependency on dream-domain-employee — the reverse would be
-- circular — and dream-domain-platform is enterprise-feature-gated, which
-- the personal edition's skill/mcp/employee registries must not be.
CREATE TABLE IF NOT EXISTS one_content_categories (
    id            TEXT PRIMARY KEY,
    tenant_id     TEXT NOT NULL,
    parent_id     TEXT,                    -- NULL = root category
    resource_type TEXT NOT NULL,           -- 'skill' | 'mcp' | 'employee'
    name          TEXT NOT NULL,
    sort_order    INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_one_content_categories_lookup
    ON one_content_categories(tenant_id, resource_type, parent_id);

CREATE TABLE IF NOT EXISTS one_content_tags (
    id            TEXT PRIMARY KEY,
    tenant_id     TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    name          TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    UNIQUE(tenant_id, resource_type, name)
);

CREATE TABLE IF NOT EXISTS one_content_tag_links (
    tag_id        TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id   TEXT NOT NULL,
    PRIMARY KEY (tag_id, resource_type, resource_id)
);

CREATE INDEX IF NOT EXISTS idx_one_content_tag_links_resource
    ON one_content_tag_links(resource_type, resource_id);
