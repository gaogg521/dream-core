-- P2-2 extension: point-to-point conversation sharing. scope='user' rows
-- carry the recipient's user id here; reads authorize on it directly
-- (narrower than tenant, which the policy gate still opens).
ALTER TABLE one_conversation_shares ADD COLUMN target_user_id TEXT;
