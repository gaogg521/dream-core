-- one-devops 004: per-requirement autopilot flag (A1 L3) (MySQL port).
-- When set, creating/assigning the requirement (or moving it into a
-- pre-dev status) auto-dispatches it to its assigned digital employee,
-- skipping the manual "派活" click. Default off preserves L1/L2 behavior.

ALTER TABLE one_requirements ADD COLUMN autopilot TINYINT(1) NOT NULL DEFAULT 0;
