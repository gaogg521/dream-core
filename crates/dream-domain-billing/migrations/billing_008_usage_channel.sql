-- P1-4 data reports: which provider/channel configuration a turn actually
-- used, recorded verbatim as `providers.id` (dream-core-db). Enterprise
-- channels get a deterministic id derived from the registry channel id —
-- `prov_chan_<channel_id>`, see
-- dream-core-system::managed_provider::provider_id_for — while personally
-- configured providers have no registry row at all. No cross-crate join is
-- done here on purpose: this crate must not depend on dream-core-db or
-- dream-domain-devops, so prefix stripping and display-name resolution are
-- left to the frontend.
--
-- Nullable: historical rows simply have no value and are NOT backfilled.
-- Queries bucket them with COALESCE(channel_id, 'unknown'), the same
-- convention by_model applies to a NULL model.
ALTER TABLE one_usage_events ADD COLUMN channel_id TEXT;

CREATE INDEX IF NOT EXISTS idx_one_usage_events_channel
    ON one_usage_events(channel_id, created_at DESC);
