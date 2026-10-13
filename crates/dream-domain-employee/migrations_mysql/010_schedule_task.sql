-- one-employee 010: the task a scheduled run performs (MySQL port).
-- See the SQLite migration for why this is its own column.

ALTER TABLE one_personal_agents ADD COLUMN schedule_task TEXT NULL;
