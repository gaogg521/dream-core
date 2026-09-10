-- Skill presentation metadata: human-facing display name and icon.
--
-- Imported skills carry a frontmatter `display_name` (the human name from the
-- source catalog — often CJK, e.g. "12306 订票助手") and may ship an icon file
-- (`_icon.svg` / `_icon.png`) beside SKILL.md. `name` stays the ASCII
-- kebab-case identity used for matching, filesystem paths and the wire API;
-- these two columns are display-only, surfaced by the skills listing so the
-- Skills Hub can render the catalog the way its source marketplace did.
ALTER TABLE skills ADD COLUMN display_name TEXT;
ALTER TABLE skills ADD COLUMN icon_file TEXT;
