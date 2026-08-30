-- Security policy baseline (E5): the "全局基线" half of the reference
-- product's two-layer model ("全局基线 / 策略模板") — a single per-tenant
-- config row admins pick a tier for, or override field by field. The
-- "策略模板" half (multiple *named* templates, independently assignable to a
-- scope narrower than the whole tenant) is NOT modeled here — this is one
-- baseline per tenant, not a template library. See
-- `PlatformService::apply_security_policy_tier`'s doc comment for the exact
-- boundary and why it's drawn there (MySQL port).
--
-- Same "reserved adapter" shape as 001_init: storing/toggling a row here does
-- NOT by itself enforce anything. `terminal_tools_require_approval` /
-- `destructive_commands_blocked` / `external_network_denied_by_default` are
-- read by nothing yet — no tool-execution path in this codebase currently
-- consults this table. Message scanning is the one exception: it's the
-- existing DLP engine (`dream_core_system::ContentInspectionService`,
-- `one_dlp_rules` in dream-domain-devops) already enforcing locally on every
-- send; `message_scan_enabled`/`message_redact_enabled` here describe
-- whether this tenant's baseline calls for a DLP rule set to be authored and
-- distributed, not a new enforcement mechanism.
CREATE TABLE IF NOT EXISTS one_security_policy (
    tenant_id                          VARCHAR(255) PRIMARY KEY NOT NULL,
    -- 'relaxed' | 'standard' | 'strict' | 'custom'. 'custom' means at least
    -- one field below was hand-edited after a tier was applied — see
    -- `PlatformService::set_security_policy`.
    tier                               VARCHAR(16) NOT NULL DEFAULT 'relaxed',
    terminal_tools_require_approval    TINYINT(1) NOT NULL DEFAULT 0,
    destructive_commands_blocked       TINYINT(1) NOT NULL DEFAULT 0,
    -- JSON array of glob/keyword patterns ('rm -rf', 'shutdown', 'mkfs',
    -- 'sudo', 'kubectl delete', ...). Only consulted when the flag above is on.
    blocked_command_patterns           TEXT NOT NULL DEFAULT ('[]'),
    external_network_denied_by_default TINYINT(1) NOT NULL DEFAULT 0,
    message_scan_enabled               TINYINT(1) NOT NULL DEFAULT 0,
    message_redact_enabled             TINYINT(1) NOT NULL DEFAULT 0,
    -- Sends per member per minute. NULL = unlimited.
    send_rate_limit_per_minute         INT NULL,
    updated_at                         BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
