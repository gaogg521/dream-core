-- How a scene turns a rough request into a real one (MySQL port). See the SQLite
-- migration for why.
ALTER TABLE one_scenes ADD COLUMN intent TEXT NULL;
