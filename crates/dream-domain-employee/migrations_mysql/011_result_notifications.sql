CREATE TABLE IF NOT EXISTS one_employee_notifications (
    agent_id VARCHAR(191) NOT NULL,
    owner_user_id VARCHAR(191) NOT NULL,
    target TEXT NOT NULL,
    PRIMARY KEY (agent_id, owner_user_id)
);
CREATE TABLE IF NOT EXISTS one_employee_deliveries (
    run_id VARCHAR(191) PRIMARY KEY,
    status VARCHAR(32) NOT NULL,
    updated_at BIGINT NOT NULL
);
