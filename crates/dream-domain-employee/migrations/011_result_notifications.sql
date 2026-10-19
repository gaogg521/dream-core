CREATE TABLE IF NOT EXISTS one_employee_notifications (
    agent_id TEXT NOT NULL,
    owner_user_id TEXT NOT NULL,
    target TEXT NOT NULL,
    PRIMARY KEY (agent_id, owner_user_id)
);
CREATE TABLE IF NOT EXISTS one_employee_deliveries (
    run_id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    updated_at BIGINT NOT NULL
);
