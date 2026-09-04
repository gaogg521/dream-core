-- Operation-audit outcome columns (align-openocta 实机复核 2026-09-02) (MySQL port).
--
--   one_audit_logs.latency_ms  wall-clock ms measured at the handler
--   one_audit_logs.result      'success' | 'failure'
--
-- `result` is VARCHAR, not TEXT: MySQL forbids a DEFAULT on TEXT/BLOB.
-- Existing rows default to 'success' (every current audit() call fires only
-- after the operation returned Ok). `latency_ms` stays nullable — no fake 0
-- for rows written before this migration.
ALTER TABLE one_audit_logs ADD COLUMN latency_ms BIGINT NULL;
ALTER TABLE one_audit_logs ADD COLUMN result VARCHAR(16) NOT NULL DEFAULT 'success';
