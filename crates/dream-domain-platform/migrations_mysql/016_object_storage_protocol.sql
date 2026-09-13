-- platform 016: which protocol a storage config speaks (MySQL port).
-- See the SQLite copy for the reasoning.
ALTER TABLE one_object_storage_configs ADD COLUMN protocol VARCHAR(16) NOT NULL DEFAULT 's3';
