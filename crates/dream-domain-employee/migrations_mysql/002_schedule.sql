-- one-employee 002: per-agent schedule for cron-driven runs (MySQL port).
-- Adds three columns to one_personal_agents so the in-crate 30s scanner
-- can pick up due schedules without touching upstream aionui-cron state.
-- schedule stores CronScheduleDto JSON (tag = "kind": at/every/cron);
-- schedule_enabled gates the scanner; next_run_at is the cached next
-- fire time (ms epoch) recomputed via aionui_cron::compute_next_run.
--
-- MySQL port note: SQLite's partial index (`WHERE schedule_enabled = 1`)
-- maps to a plain index — a scan by (schedule_enabled, next_run_at) is
-- equally served, and the predicate's selectivity is enforced by the
-- scanner's own query, which filters `schedule_enabled = 1` anyway.

ALTER TABLE one_personal_agents ADD COLUMN schedule TEXT NULL;
ALTER TABLE one_personal_agents ADD COLUMN schedule_enabled TINYINT(1) NOT NULL DEFAULT 0;
ALTER TABLE one_personal_agents ADD COLUMN next_run_at BIGINT NULL;
CREATE INDEX idx_one_personal_agents_schedule
    ON one_personal_agents (schedule_enabled, next_run_at);
