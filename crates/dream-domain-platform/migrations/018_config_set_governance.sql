-- Config-set governance fields that OpenOcta's vault list actually stores.
-- `name` remains the unique consumer alias for {{config.<name>.<key>}}.
-- `alias` is an optional display handle (empty = show name).
-- `template` is a free-form template/kind label, not a second vault.
-- `scope` is tenant | department | personal.
-- `enabled` gates runtime expansion of {{config.*}} tokens.
ALTER TABLE one_config_sets ADD COLUMN alias TEXT NOT NULL DEFAULT '';
ALTER TABLE one_config_sets ADD COLUMN template TEXT NOT NULL DEFAULT '';
ALTER TABLE one_config_sets ADD COLUMN scope TEXT NOT NULL DEFAULT 'tenant';
ALTER TABLE one_config_sets ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
