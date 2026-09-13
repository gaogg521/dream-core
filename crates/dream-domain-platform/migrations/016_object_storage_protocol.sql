-- platform 016: which protocol a storage config speaks.
--
-- 015 assumed S3 because that is what every public cloud and every
-- self-hosted gateway (MinIO, Ceph) implements. An enterprise NAS does not —
-- it speaks SMB/NFS/WebDAV — so "object storage" was quietly narrower than
-- "the storage this company already owns".
--
-- Existing rows are S3 by definition: that is the only driver 015 could talk
-- to, so the default backfills them correctly.
ALTER TABLE one_object_storage_configs ADD COLUMN protocol TEXT NOT NULL DEFAULT 's3';
