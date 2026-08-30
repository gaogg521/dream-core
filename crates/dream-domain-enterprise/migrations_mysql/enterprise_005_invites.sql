-- Pending company invites: an admin picks a person from the synced Feishu
-- directory and gets a shareable link/code to hand them (MySQL port).
--
-- Deliberately NOT an access gate — `sync_member` (SSO login) still
-- auto-joins ANY successful login to the sole manual company, unchanged.
-- This table exists purely so the admin can (a) see "invited, not yet
-- joined" people in the Members tab before they log in, and (b) hand out a
-- link pre-labelled with who it's for. A row here is consumed (deleted) the
-- moment that same (provider, external_id) actually completes SSO login.
CREATE TABLE IF NOT EXISTS one_enterprise_invites (
    id            VARCHAR(255) PRIMARY KEY,
    enterprise_id VARCHAR(255) NOT NULL,
    -- Same identity space as one_enterprises/one_enterprise_members: the IdP
    -- provider and the invitee's opaque IdP id (Feishu union_id etc.).
    provider      VARCHAR(64) NOT NULL,
    external_id   VARCHAR(255) NOT NULL,
    -- Directory-sourced display fields, shown in the Members tab's pending
    -- row before the real SSO login (which will bring its own, authoritative
    -- copies) exists.
    display_name  VARCHAR(255) NULL,
    department    TEXT NULL,
    job_title     VARCHAR(255) NULL,
    created_by    VARCHAR(255) NOT NULL,
    created_at    BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;

-- Re-inviting the same directory person replaces their pending invite rather
-- than piling up duplicates.
CREATE UNIQUE INDEX idx_one_enterprise_invites_person
    ON one_enterprise_invites (enterprise_id, provider, external_id);

CREATE INDEX idx_one_enterprise_invites_enterprise
    ON one_enterprise_invites (enterprise_id);
