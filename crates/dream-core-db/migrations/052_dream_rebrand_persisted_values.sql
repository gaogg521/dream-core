-- Forward migration for the Dream platform rebrand: convert persisted
-- 'aionrs' / 'aionui' string values (originally from the aionrs-local /
-- AionUi upstream) to the new 'dream' identity. Historical migrations
-- (001, 013, 015, 016, 019, 021, ...) are never rewritten; this migration
-- only updates existing rows going forward, per the one-time-migration
-- principle in DREAM-PLATFORM-DIRECTION.md section 4.1.
--
-- Scope: only the AgentType ('aionrs' -> 'dream') and ConversationSource
-- ('aionui' -> 'dream') wire values. Protocol-level identifiers that are
-- deliberately NOT migrated (appId, exe name, userData dir, deep-link
-- scheme, JWT issuer/audience, OAuth client_id, cache directory names)
-- are covered separately — see section 12/13 of the decision document.

-- NOTE: `assistants.preset_agent_type` / `assistant_overrides.preset_agent_type`
-- and `cron_jobs.agent_type` from the 001 snapshot were dropped by later
-- rebuilds (013's table-rebuild pattern) and folded into JSON blob columns
-- (`assistant_definitions`'s generated-assistant JSON, `cron_jobs.agent_config`).
-- Only plain TEXT columns confirmed present in the current schema are
-- migrated here; the JSON-embedded occurrences are a known, lower-risk
-- residual (they don't block deserialization the way a plain column does,
-- since callers read them through their own JSON parsing, not through the
-- AgentType enum's Deserialize impl) — see DREAM-SETUP-NOTES.md.

UPDATE conversations SET type = 'dream' WHERE type = 'aionrs';
UPDATE conversations SET source = 'dream' WHERE source = 'aionui';

UPDATE agent_metadata SET agent_type = 'dream' WHERE agent_type = 'aionrs';

UPDATE assistant_sessions SET agent_type = 'dream' WHERE agent_type = 'aionrs';
