-- A first-factor challenge belongs to the session generation authenticated
-- when it was issued. Existing challenges have no trustworthy snapshot.
ALTER TABLE mfa_challenges ADD COLUMN session_generation INTEGER NOT NULL DEFAULT -1;
UPDATE mfa_challenges SET used = 1, pending_secret_cipher = NULL;
