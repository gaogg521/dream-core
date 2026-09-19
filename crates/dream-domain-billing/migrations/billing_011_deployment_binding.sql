-- Stable installation identity for request-bound offline licenses.
-- The fingerprint is random and installation-scoped; it is intentionally not
-- derived from a user, tenant name, IP address, or mutable hardware data.
CREATE TABLE IF NOT EXISTS one_license_installation (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    fingerprint  TEXT    NOT NULL,
    created_at   INTEGER NOT NULL
);

ALTER TABLE one_license_activation ADD COLUMN deployment_fingerprint TEXT;
