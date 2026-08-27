-- Forward migration for the last upstream-branded identifiers a fresh install
-- would still receive: the built-in butler assistant and the four skills it
-- ships with.
--
--   aionui-assistant      -> one-assistant
--   aionui-config         -> one-config
--   aionui-troubleshooting-> one-troubleshooting
--   aionui-webui-public   -> one-webui-public
--   aionui-webui-setup    -> one-webui-setup
--
-- Unlike the Electron-side legacy catalog (whose migrations only ever run on a
-- database that predates the backend handoff), dream-core's migrations run on
-- every new database too — so without this rename a brand-new user still gets
-- `aionui-assistant` in their assistant list and `.dream/skills/aionui-config`
-- in their workspace file tree.
--
-- Published migrations are never rewritten (018 still identifies the butler by
-- its old source_ref, and 035's comment still names the old crate). This is the
-- forward pass that moves existing rows.
--
-- Ordering note: the manifest in `assets/builtin-assistants/assistants.json` is
-- re-seeded on startup under the NEW ids. Rows renamed here therefore match the
-- manifest again on the next boot; rows this migration missed would be seeded a
-- SECOND time under the new id, leaving the user with two butlers. That is what
-- makes covering `source_ref` (the manifest's identity column, unique together
-- with `source`) the load-bearing part.

-- ---------------------------------------------------------------------------
-- Assistant definitions: identity, rule/avatar asset refs, and the JSON skill
-- lists. The JSON columns are plain TEXT arrays, so the replacement targets the
-- quoted token — matching `"aionui-config"` rather than the bare name keeps it
-- from touching a longer id that merely starts the same way.
-- ---------------------------------------------------------------------------
UPDATE assistant_definitions
SET assistant_id = 'one-assistant'
WHERE assistant_id = 'aionui-assistant';

UPDATE assistant_definitions
SET source_ref = 'one-assistant'
WHERE source = 'builtin' AND source_ref = 'aionui-assistant';

UPDATE assistant_definitions
SET rule_resource_ref = 'one-assistant'
WHERE rule_resource_ref = 'aionui-assistant';

UPDATE assistant_definitions
SET avatar_value = 'avatars/one-assistant.jpg'
WHERE avatar_value = 'avatars/aionui-assistant.jpg';

UPDATE assistant_definitions
SET default_skill_ids = REPLACE(
        REPLACE(
            REPLACE(
                REPLACE(default_skill_ids, '"aionui-troubleshooting"', '"one-troubleshooting"'),
                '"aionui-webui-public"', '"one-webui-public"'
            ),
            '"aionui-webui-setup"', '"one-webui-setup"'
        ),
        '"aionui-config"', '"one-config"'
    )
WHERE default_skill_ids LIKE '%"aionui-%';

UPDATE assistant_definitions
SET default_disabled_builtin_skill_ids = REPLACE(
        REPLACE(
            REPLACE(
                REPLACE(default_disabled_builtin_skill_ids, '"aionui-troubleshooting"', '"one-troubleshooting"'),
                '"aionui-webui-public"', '"one-webui-public"'
            ),
            '"aionui-webui-setup"', '"one-webui-setup"'
        ),
        '"aionui-config"', '"one-config"'
    )
WHERE default_disabled_builtin_skill_ids LIKE '%"aionui-%';

UPDATE assistant_definitions
SET custom_skill_names = REPLACE(
        REPLACE(
            REPLACE(
                REPLACE(custom_skill_names, '"aionui-troubleshooting"', '"one-troubleshooting"'),
                '"aionui-webui-public"', '"one-webui-public"'
            ),
            '"aionui-webui-setup"', '"one-webui-setup"'
        ),
        '"aionui-config"', '"one-config"'
    )
WHERE custom_skill_names LIKE '%"aionui-%';

-- ---------------------------------------------------------------------------
-- Legacy per-user enable/disable mirror. Its primary key IS the assistant id
-- (see 018), so leaving it behind would strand the user's butler enable state
-- on a row nothing looks up any more. `OR IGNORE` covers the case where a row
-- under the new id somehow already exists — keeping the newer one rather than
-- failing the whole migration.
-- ---------------------------------------------------------------------------
UPDATE OR IGNORE assistant_overrides
SET assistant_id = 'one-assistant'
WHERE assistant_id = 'aionui-assistant';

-- ---------------------------------------------------------------------------
-- Scheduled tasks bind their assistant inside a JSON blob rather than a column.
-- A job left pointing at the old id resolves to nothing and fails at run time,
-- long after the upgrade, with no obvious connection to it.
-- ---------------------------------------------------------------------------
UPDATE cron_jobs
SET agent_config = json_set(agent_config, '$.assistant_id', 'one-assistant')
WHERE json_valid(agent_config)
  AND json_extract(agent_config, '$.assistant_id') = 'aionui-assistant';

-- ---------------------------------------------------------------------------
-- Skill rows. Renaming rather than deleting preserves the user's per-skill
-- enable/disable toggle; `path` still points at the old directory afterwards,
-- which the startup builtin-skill sync corrects (it reconciles by name).
-- `OR IGNORE` guards the UNIQUE(name) constraint.
-- ---------------------------------------------------------------------------
UPDATE OR IGNORE skills SET name = 'one-config' WHERE name = 'aionui-config' AND source = 'builtin';
UPDATE OR IGNORE skills SET name = 'one-troubleshooting' WHERE name = 'aionui-troubleshooting' AND source = 'builtin';
UPDATE OR IGNORE skills SET name = 'one-webui-public' WHERE name = 'aionui-webui-public' AND source = 'builtin';
UPDATE OR IGNORE skills SET name = 'one-webui-setup' WHERE name = 'aionui-webui-setup' AND source = 'builtin';
