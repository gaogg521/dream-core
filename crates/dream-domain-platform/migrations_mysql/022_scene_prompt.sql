-- A scene's own prompt (MySQL port). See the SQLite migration for why.
ALTER TABLE one_scenes ADD COLUMN prompt TEXT NULL;
