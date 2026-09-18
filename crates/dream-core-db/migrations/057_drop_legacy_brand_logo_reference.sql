-- Repoint anything still referencing the pre-rebrand brand logo.
--
-- Migration 001 seeded the internal agent with `/api/assets/logos/brand/aion.svg`
-- and 021 repointed it to the 1ONE mascot — but only for rows matching its narrow
-- WHERE (agent_type = 'aionrs' AND agent_source = 'internal', and
-- source = 'generated' for assistants). Any row outside those predicates — a
-- user-edited agent, an assistant created from a different source — kept the old
-- path. This sweeps whatever is left so the asset itself can stop shipping.
--
-- Deliberately matches on the PATH rather than a row id: the point is to leave no
-- reference to that file anywhere, not to fix one known row.

UPDATE agent_metadata
SET icon = '/api/assets/logos/brand/1one.png',
    updated_at = unixepoch('now', 'subsec') * 1000
WHERE icon = '/api/assets/logos/brand/aion.svg';

UPDATE assistant_definitions
SET avatar_type = 'builtin_asset',
    avatar_value = '/api/assets/logos/brand/1one.png',
    updated_at = unixepoch('now', 'subsec') * 1000
WHERE avatar_value = '/api/assets/logos/brand/aion.svg';
