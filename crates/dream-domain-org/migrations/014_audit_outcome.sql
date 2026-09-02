-- Operation-audit outcome columns (align-openocta 实机复核 2026-09-02).
--
-- The audit tab listed time / actor / action / resource / IP; the reference
-- product also shows how long the action took and whether it succeeded.
--
--   one_audit_logs.latency_ms  wall-clock ms measured at the handler
--   one_audit_logs.result      'success' | 'failure'
--
-- Existing rows read as `result = 'success'`: every current `audit()` call
-- site fires only AFTER the operation returned Ok (past the `?`), so the
-- default is the truthful value for历史 data. `latency_ms` is nullable —
-- rows written before this migration, and the handful of call sites that
-- still don't time themselves, carry no measurement rather than a fake 0.
ALTER TABLE one_audit_logs ADD COLUMN latency_ms INTEGER;
ALTER TABLE one_audit_logs ADD COLUMN result TEXT NOT NULL DEFAULT 'success';
