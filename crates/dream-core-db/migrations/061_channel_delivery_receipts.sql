CREATE TABLE IF NOT EXISTS assistant_delivery_receipts (
 owner_user_id TEXT NOT NULL,
 platform TEXT NOT NULL,
 delivery_id TEXT NOT NULL,
 received_at INTEGER NOT NULL,
 PRIMARY KEY(owner_user_id, platform, delivery_id)
);
CREATE INDEX IF NOT EXISTS idx_assistant_delivery_receipts_time ON assistant_delivery_receipts(received_at);
