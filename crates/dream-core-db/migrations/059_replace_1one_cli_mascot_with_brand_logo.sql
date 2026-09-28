-- Use the 1ONE CLI product mark instead of the mascot for the internal CLI.
-- The generated assistant is a derived projection of this row, but update its
-- stored snapshot as well so existing users see the new identity immediately.

UPDATE agent_metadata
SET icon = '/api/assets/logos/brand/1one-cli.png',
    updated_at = unixepoch('now', 'subsec') * 1000
WHERE agent_id = '632f31d2'
  AND agent_source = 'internal'
  AND icon IS NOT '/api/assets/logos/brand/1one-cli.png';

UPDATE assistant_definitions
SET avatar_type = 'builtin_asset',
    avatar_value = '/api/assets/logos/brand/1one-cli.png',
    updated_at = unixepoch('now', 'subsec') * 1000
WHERE source = 'generated'
  AND EXISTS (
      SELECT 1
      FROM agent_metadata am
      WHERE am.id = assistant_definitions.source_ref
        AND am.agent_id = '632f31d2'
        AND am.agent_source = 'internal'
  )
  AND (
      avatar_type IS NOT 'builtin_asset'
      OR avatar_value IS NOT '/api/assets/logos/brand/1one-cli.png'
  );
