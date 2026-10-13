-- Where a scheduled run's result is pushed once it finishes: JSON
-- {"plugin_id": "...", "chat_id": "..."} naming one of the owner's channel bots
-- (and the chat to message). NULL means the result stays in the app, as before.
ALTER TABLE cron_jobs ADD COLUMN push_target TEXT;
