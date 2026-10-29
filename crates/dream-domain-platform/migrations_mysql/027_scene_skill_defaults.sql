CREATE TABLE IF NOT EXISTS one_scene_skill_defaults (tenant_id VARCHAR(191) NOT NULL, scene_id VARCHAR(191) NOT NULL, applied_at BIGINT NOT NULL, PRIMARY KEY (tenant_id, scene_id));
