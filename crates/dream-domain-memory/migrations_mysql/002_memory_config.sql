-- Enterprise memory config (P2-2 followups §A.6): per-tenant extraction
-- settings for the LLM-backed turn extractor.
--
-- One row per tenant, created lazily on first save — absence of a row means
-- "extraction disabled": the turn extractor keeps honouring explicit
-- 「记住…」 requests but never invokes an LLM. `extraction_channel_id`
-- references one-devops' `one_provider_registry` (a company model channel —
-- the credential stays there, encrypted; this table stores only the id).
-- `extraction_model` names the channel model to call; empty = the channel's
-- first configured model.

CREATE TABLE IF NOT EXISTS one_memory_config (
    tenant_id            VARCHAR(255) PRIMARY KEY NOT NULL,
    extraction_channel_id VARCHAR(255) NULL,
    extraction_model     VARCHAR(255) NULL,
    updated_at           BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
