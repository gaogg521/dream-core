-- Scene management (E5 "场景管理"): a named, reusable bundle of resource
-- grants + descriptive job-function tags. A member added to a scene reaches
-- everything the scene has been granted (`one_resource_grants` with
-- `subject_type = 'scene', subject_id = one_scenes.id`), without an admin
-- re-granting the same skill/tool/model/channel set to every new hire one at
-- a time.
--
-- Deliberately NOT modeled here: pre-populated grants for the 5 reference
-- built-in scenes (办公/IT运维/网络安全/新媒体运营/市场营销). Those grants
-- would have to name specific skill/MCP/model-channel resource ids, and
-- those don't exist in a fresh install — they're admin- or member-created at
-- runtime, not a fixed catalog this product ships. The 5 built-ins seeded
-- below are named, empty templates an admin fills in via the same
-- resource-grants endpoints (`subject_type=scene`); see
-- `PlatformService::seed_builtin_scenes`.
CREATE TABLE IF NOT EXISTS one_scenes (
    id            TEXT    PRIMARY KEY NOT NULL,
    tenant_id     TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    description   TEXT,
    -- JSON array of free-text job-function labels ("职能") — purely
    -- descriptive metadata shown in the admin UI, no enforcement behavior.
    job_functions TEXT    NOT NULL DEFAULT '[]',
    -- A built-in scene can be edited (including its resource grants) but not
    -- deleted — see `PlatformService::delete_scene`.
    built_in      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    UNIQUE(tenant_id, name)
);

-- Membership: which members belong to which scene. Many-to-many — a member
-- can hold more than one scene (job functions can span, e.g. someone doing
-- both IT ops and security).
CREATE TABLE IF NOT EXISTS one_scene_members (
    scene_id  TEXT    NOT NULL,
    tenant_id TEXT    NOT NULL,
    user_id   TEXT    NOT NULL,
    added_at  INTEGER NOT NULL,
    PRIMARY KEY (scene_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_one_scene_members_member
    ON one_scene_members(tenant_id, user_id);
