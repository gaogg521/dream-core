-- SCIM 2.0 inbound provisioning (MySQL).

CREATE TABLE IF NOT EXISTS one_scim_settings (
    id TINYINT PRIMARY KEY,
    token_sha256 VARCHAR(64) NOT NULL,
    enabled TINYINT(1) NOT NULL DEFAULT 1,
    updated_at BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_scim_users (
    id VARCHAR(255) PRIMARY KEY,
    external_id VARCHAR(255) NOT NULL,
    user_id VARCHAR(255) NOT NULL,
    user_name VARCHAR(255) NOT NULL,
    display_name VARCHAR(255) NULL,
    active TINYINT(1) NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    UNIQUE KEY uq_one_scim_users_external (external_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE INDEX idx_one_scim_users_user_id ON one_scim_users (user_id);

CREATE TABLE IF NOT EXISTS one_scim_groups (
    id VARCHAR(255) PRIMARY KEY,
    external_id VARCHAR(255) NULL,
    display_name VARCHAR(255) NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    UNIQUE KEY uq_one_scim_groups_external (external_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

CREATE TABLE IF NOT EXISTS one_scim_group_members (
    group_id VARCHAR(255) NOT NULL,
    user_scim_id VARCHAR(255) NOT NULL,
    PRIMARY KEY (group_id, user_scim_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
