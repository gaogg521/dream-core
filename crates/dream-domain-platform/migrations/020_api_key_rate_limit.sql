-- Shared counters keep app and admin processes within one per-key limit.
ALTER TABLE one_api_keys ADD COLUMN rate_window_start INTEGER NOT NULL DEFAULT 0;
ALTER TABLE one_api_keys ADD COLUMN rate_window_count INTEGER NOT NULL DEFAULT 0;
