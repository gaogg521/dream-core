-- P1-1 round 1 (align-openocta §4): shared category tree + tag tables for the
-- three content-registry resource types (skill / mcp / employee). One shared
-- table set filtered by `resource_type` rather than three separate schemas —
-- unlike the employee-grants split (migration 006), all three resource
-- shapes here are identical ("attach a category + zero or more tags to an
-- id"), so there's no semantic conflict that would justify separate tables
-- (MySQL port).
--
-- Lives in dream-domain-employee (not dream-domain-devops, and not
-- dream-domain-platform) because dream-domain-devops already has a real
-- Cargo dependency on dream-domain-employee — the reverse would be
-- circular — and dream-domain-platform is enterprise-feature-gated, which
-- the personal edition's skill/mcp/employee registries must not be.
CREATE TABLE IF NOT EXISTS one_content_categories (
    id            VARCHAR(255) PRIMARY KEY,
    tenant_id     VARCHAR(255) NOT NULL,
    parent_id     VARCHAR(255) NULL,      -- NULL = root category
    resource_type VARCHAR(32) NOT NULL,   -- 'skill' | 'mcp' | 'employee'
    name          VARCHAR(255) NOT NULL,
    sort_order    BIGINT NOT NULL DEFAULT 0,
    created_at    BIGINT NOT NULL,
    updated_at    BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_content_categories_lookup
    ON one_content_categories (tenant_id, resource_type, parent_id);

CREATE TABLE IF NOT EXISTS one_content_tags (
    id            VARCHAR(255) PRIMARY KEY,
    tenant_id     VARCHAR(255) NOT NULL,
    resource_type VARCHAR(32) NOT NULL,
    name          VARCHAR(191) NOT NULL,
    created_at    BIGINT NOT NULL,
    UNIQUE KEY uq_one_content_tags (tenant_id, resource_type, name)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_content_tag_links (
    tag_id        VARCHAR(255) NOT NULL,
    resource_type VARCHAR(32) NOT NULL,
    resource_id   VARCHAR(255) NOT NULL,
    PRIMARY KEY (tag_id, resource_type, resource_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE INDEX idx_one_content_tag_links_resource
    ON one_content_tag_links (resource_type, resource_id);
