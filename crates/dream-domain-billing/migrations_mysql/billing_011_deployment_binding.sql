CREATE TABLE IF NOT EXISTS one_license_installation (
    singleton_id TINYINT PRIMARY KEY NOT NULL,
    fingerprint  VARCHAR(80) NOT NULL,
    created_at   BIGINT NOT NULL,
    CONSTRAINT chk_one_license_installation_singleton CHECK (singleton_id = 1)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

ALTER TABLE one_license_activation ADD COLUMN deployment_fingerprint VARCHAR(80) NULL;
