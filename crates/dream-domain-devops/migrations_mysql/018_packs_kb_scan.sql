-- MCP pack content, knowledge libraries, scan policy (MySQL).
ALTER TABLE one_mcp_registry ADD COLUMN content LONGTEXT NOT NULL DEFAULT '';

CREATE TABLE IF NOT EXISTS one_rag_libraries (
    id          VARCHAR(64)  PRIMARY KEY NOT NULL,
    name        VARCHAR(255) NOT NULL,
    description TEXT         NOT NULL,
    created_by  VARCHAR(64)  NOT NULL,
    created_at  BIGINT       NOT NULL,
    updated_at  BIGINT       NOT NULL
);

INSERT INTO one_rag_libraries (id, name, description, created_by, created_at, updated_at)
VALUES ('oraglib_default', '默认知识库', '系统自动创建的知识库容器', 'system', 0, 0);

ALTER TABLE one_rag_documents ADD COLUMN library_id VARCHAR(64) NOT NULL DEFAULT 'oraglib_default';

CREATE TABLE IF NOT EXISTS one_scan_policy (
    id                VARCHAR(32) PRIMARY KEY NOT NULL,
    block_on_warning  TINYINT(1)  NOT NULL DEFAULT 0,
    extra_needles     TEXT        NOT NULL,
    updated_at        BIGINT      NOT NULL
);

INSERT INTO one_scan_policy (id, block_on_warning, extra_needles, updated_at)
VALUES ('default', 0, '[]', 0);
