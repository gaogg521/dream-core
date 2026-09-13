-- Forward-only visual identity for work scenes (MySQL port).
ALTER TABLE one_scenes ADD COLUMN avatar_ref MEDIUMTEXT NULL AFTER description;
