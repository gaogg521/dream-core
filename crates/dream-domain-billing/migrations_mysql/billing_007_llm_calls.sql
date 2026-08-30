-- one-billing P2-5: per-model-call LLM trace ("逐次 LLM Trace", aligned with
-- OpenOcta's per-call timeline) (MySQL port).
--
-- Granularity vs `one_usage_events` (billing_001): that table is one row per
-- COMPLETED AGENT TURN (aggregated, priced, and fed into spend caps and the
-- usage dashboard). One turn can contain several model calls — tool rounds,
-- vision delegates, error retries — each billed by the provider separately.
-- `one_llm_calls` is the fine-grained view: one row per MODEL CALL, carrying
-- tokens, duration, the owning tool round, and the failure reason. It is a
-- diagnostic/observability surface only: nothing here feeds budgets or cost
-- estimates (those stay on `one_usage_events`), and no prompt/response content
-- is stored — same privacy posture as the per-turn table.
--
-- Retention: raw call rows are high-volume and only useful for recent
-- debugging; the durable aggregate already lives in `one_usage_events`. Rows
-- older than the default window (`LLM_CALL_RETENTION_DAYS`, 30 days — matching
-- the 30-day default window every other billing dashboard query uses) are
-- deleted via `purge_llm_calls_older_than` /
-- `POST /api/one/billing/llm-calls/purge`. No automatic sweeper runs; purge is
-- invoked explicitly (admin endpoint / scheduled job at the wiring layer).

CREATE TABLE IF NOT EXISTS one_llm_calls (
    id              VARCHAR(255) PRIMARY KEY NOT NULL,
    enterprise_id   VARCHAR(255) NOT NULL,   -- same tenant dimension as one_usage_events
    user_id         VARCHAR(255) NOT NULL,
    conversation_id VARCHAR(255) NULL,
    model           VARCHAR(255) NULL,
    provider        VARCHAR(64) NULL,        -- where the call's shape came from: acp / dream_engine / direct_cli
    tool_name       VARCHAR(255) NULL,       -- the tool round this call belonged to (nullable)
    input_tokens    BIGINT NOT NULL DEFAULT 0,
    output_tokens   BIGINT NOT NULL DEFAULT 0,
    duration_ms     BIGINT NULL,
    -- NULL = the call succeeded; otherwise the failure reason. Failed calls
    -- are recorded like successful ones: a retry storm is exactly what this
    -- trace exists to expose.
    error           TEXT NULL,
    created_at      BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_llm_calls_ent  ON one_llm_calls (enterprise_id, created_at DESC);
CREATE INDEX idx_one_llm_calls_user ON one_llm_calls (user_id, created_at DESC);
