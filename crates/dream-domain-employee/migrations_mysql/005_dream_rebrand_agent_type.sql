-- See dream-core-db/migrations/052_dream_rebrand_persisted_values.sql for
-- the full rationale. This crate stores its own copy of the agent_type
-- value on digital-employee agent rows.

UPDATE one_personal_agents SET agent_type = 'dream' WHERE agent_type = 'aionrs';
