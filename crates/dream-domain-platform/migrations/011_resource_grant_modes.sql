-- How the resource matrix (003) is read, per tenant and per resource type.
--
-- The matrix has always been additive: a grant can only ADD reachability on
-- top of each registry's own `scope`/`visibility` columns, never take any
-- away. That is what made shipping it safe — an install that never touched
-- the matrix ran bit-for-bit the query it ran before.
--
-- It is also not what an administrator assumes. Someone who opens the matrix
-- and grants a department exactly three skills reads that as a whitelist, and
-- is not told that the department still reaches every `visibility = 'all'`
-- skill in its scopes. This table lets a tenant say "for this resource type,
-- granted means ONLY the granted ones".
--
-- Deliberately a separate table with no rows by default, rather than a column
-- with a default on some existing settings row:
--
--   * absent row = additive = today's behaviour, so every existing install is
--     unaffected until an admin opts in, per resource type;
--   * `one_security_policy` is snapshotted field-by-field by the policy
--     template layer (009), so an eighth field there would have to be threaded
--     through template create/apply as well — this is not that kind of setting;
--   * restrictive mode is a read-path semantic, so it belongs next to the
--     grants whose reading it changes.
--
-- Enforcement lives in dream-domain-devops (`apply_grants`), which learns the
-- mode through the `ResourceGrantSource` seam — it never reads this table, and
-- must not: the personal edition compiles that crate without this one.
--
-- SAFETY: every failure path on the way to reading this (no tenant, matrix
-- unreadable, this table unreadable) resolves to additive, never restrictive.
-- A transient database error must not blank out every member's skill list.
CREATE TABLE IF NOT EXISTS one_resource_grant_modes (
    tenant_id     TEXT    NOT NULL,
    resource_type TEXT    NOT NULL,  -- 'skill' | 'mcp' | 'knowledge' | 'model_channel'
    mode          TEXT    NOT NULL,  -- 'additive' | 'restrictive'
    updated_by    TEXT    NOT NULL,
    updated_at    INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, resource_type)
);
