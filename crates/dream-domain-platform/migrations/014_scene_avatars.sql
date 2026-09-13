-- Forward-only visual identity for work scenes.  Values may be a bundled
-- asset key, an https URL, or a small data:image/png URL supplied by admin.
ALTER TABLE one_scenes ADD COLUMN avatar_ref TEXT;
