-- Member-facing memory management: per-member deletion rights and a recall
-- opt-out (see the member routes under /api/one/memory/*).
--
-- `author_user_id` records which member wrote an item through the member
-- routes; it is what lets a member delete "their own" items inside
-- collections they cannot manage. NULL for rows written by admins, the turn
-- extractor, or refinement merges — nobody but an admin may delete those.
-- The column is appended (never NOT NULL) because existing installs already
-- carry rows; the migration must be shape-only and instant.
--
-- `one_memory_member_prefs` is the member's recall opt-out. Absent row =
-- recall enabled (opt-out model, not opt-in). The recall path reads this
-- fail-CLOSED: a read error skips injection rather than injecting against an
-- explicit member preference. Note this is the opposite direction of
-- `one_resource_grant_modes`' fail-additive rule, on purpose: there the
-- failure widens towards the historical behaviour; here the preference is a
-- privacy decision, and transiently ignoring it is worse than transiently
-- losing recall. Deliberately NOT a column on `one_security_policy`: the
-- policy template layer snapshots that table field-by-field (see migration
-- 011's reasoning in dream-domain-platform), and this is a per-member row,
-- not a per-tenant setting.

ALTER TABLE one_memory_items ADD COLUMN author_user_id TEXT;

CREATE TABLE IF NOT EXISTS one_memory_member_prefs (
    tenant_id      TEXT    NOT NULL,
    user_id        TEXT    NOT NULL,
    recall_enabled INTEGER NOT NULL DEFAULT 1,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, user_id)
);
