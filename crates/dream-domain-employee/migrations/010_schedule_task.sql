-- one-employee 010: the task a scheduled run performs.
--
-- A scheduled run used to send `automation_config.instructions`, falling back
-- to "carry out your daily duties" when that was empty. A company-distributed
-- employee keeps its identity in `persona` and has no instructions, so a
-- schedule on it produced a run with no task at all. The task a member
-- schedules is the member's own, so it lives beside the schedule rather than
-- in `automation_config`, which the team sync rewrites from the server on
-- every cycle (and which it would have wiped).

ALTER TABLE one_personal_agents ADD COLUMN schedule_task TEXT;
