-- MCP pack content, knowledge libraries, scan policy.
ALTER TABLE one_mcp_registry ADD COLUMN content TEXT NOT NULL DEFAULT '';

CREATE TABLE IF NOT EXISTS one_rag_libraries (
    id          TEXT    PRIMARY KEY NOT NULL,
    name        TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    created_by  TEXT    NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

INSERT INTO one_rag_libraries (id, name, description, created_by, created_at, updated_at)
VALUES ('oraglib_default', '默认知识库', '系统自动创建的知识库容器', 'system', 0, 0);

ALTER TABLE one_rag_documents ADD COLUMN library_id TEXT NOT NULL DEFAULT 'oraglib_default';

CREATE TABLE IF NOT EXISTS one_scan_policy (
    id                TEXT    PRIMARY KEY NOT NULL,
    block_on_warning  INTEGER NOT NULL DEFAULT 0,
    extra_needles     TEXT    NOT NULL DEFAULT '[]',
    updated_at        INTEGER NOT NULL
);

INSERT INTO one_scan_policy (id, block_on_warning, extra_needles, updated_at)
VALUES ('default', 0, '[]', 0);
