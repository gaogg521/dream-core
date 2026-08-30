-- Runtime-node control plane (P1-7, align-openocta 智能体节点控制面).
--
-- Three additions to the heartbeat-driven roster:
--
--   one_runtime_nodes.status      'approved' | 'pending' | 'blocked'
--   one_runtime_nodes.visibility  'private' | 'shared'
--   one_runtime_policy            per-tenant require_approval switch
--
-- The defaults are chosen so that EXISTING deployments see byte-for-byte
-- the behavior they had before this migration: every current row reads as
-- approved (no retroactive restriction — the gate-semantics red line),
-- every row reads as private (the roster was admin-only anyway), and a
-- tenant with no policy row runs in open mode (first heartbeat
-- auto-approves, exactly as before). `require_approval` is the opt-in:
-- once flipped, a FIRST-SEEN machine registers as `pending` and an access
-- review task is raised; pending nodes keep heartbeating (their row
-- updates — the machine is healthy, the review is organizational) and a
-- BLOCKED machine's heartbeat is refused outright, which is the one real
-- enforcement point that exists (the roster is a visibility surface; it
-- gates nothing else today).
--
-- 转私有/转公有 (visibility): a node belongs to the member whose machine
-- reported it. `shared` marks it as organizational infrastructure visible
-- beyond its owner. The consumer for the member-facing view is the desktop
-- client (dream-ui) and is a documented follow-up; the admin console sees
-- every node either way.
ALTER TABLE one_runtime_nodes ADD COLUMN status TEXT NOT NULL DEFAULT 'approved';
ALTER TABLE one_runtime_nodes ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private';

CREATE TABLE IF NOT EXISTS one_runtime_policy (
    tenant_id        TEXT PRIMARY KEY,
    require_approval INTEGER NOT NULL DEFAULT 0,
    updated_at       INTEGER NOT NULL
);
