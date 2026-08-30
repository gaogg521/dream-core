-- T7: per-department spend caps and usage attribution (MySQL port).
--
-- `one_usage_events.department_id` is stamped at record time from the user's
-- CURRENT department (`one_user_org.department_id`, owned by one-org — read
-- via a raw cross-table query, the same cross-crate idiom this crate already
-- uses for `one_enterprise_members`/`one_user_org`). Denormalized rather than
-- joined at query time on purpose: a department is a cost CENTER, and moving
-- someone to a different department later must not retroactively reshuffle
-- which budget last month's spend counted against.
ALTER TABLE one_usage_events ADD COLUMN department_id VARCHAR(255) NULL;
CREATE INDEX idx_one_usage_events_department ON one_usage_events (department_id, created_at DESC);

-- One row per department that has ever had a cap set. No cap = the row
-- either doesn't exist or has NULL cost_cap_micros; both mean "ungoverned at
-- the department level" (only the company-wide cap, if any, applies).
--
-- `enterprise_id` is redundant with department ownership (one-org would know
-- which company a department belongs to) but kept here so every query this
-- crate runs stays scoped to a single company without depending on one-org —
-- the same reason `one_usage_events.enterprise_id` is stored rather than
-- derived from `user_id` on every read.
CREATE TABLE IF NOT EXISTS one_department_budgets (
    department_id   VARCHAR(255) PRIMARY KEY,
    enterprise_id   VARCHAR(255) NOT NULL,
    cost_cap_micros BIGINT NULL,
    updated_at      BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_department_budgets_ent ON one_department_budgets (enterprise_id);
