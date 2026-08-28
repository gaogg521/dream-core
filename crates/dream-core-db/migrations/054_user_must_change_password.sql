-- Marks an account whose password was set by the system rather than chosen
-- by the person who owns it (the enterprise-deployment first-boot bootstrap:
-- a random password generated for the seeded admin account and logged once —
-- see `dream-core-app::services::AppServices::from_config`). While set, the
-- governance routes reject requests from that account until it changes its
-- own password via `POST /api/auth/change-password`, which clears the flag.
--
-- Default 0 for every existing row: this is additive, not a new restriction —
-- an account that already has a real, user-chosen password is never affected.
ALTER TABLE users ADD COLUMN must_change_password INTEGER NOT NULL DEFAULT 0;
