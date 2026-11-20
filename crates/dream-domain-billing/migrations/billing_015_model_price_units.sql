-- one-billing 015: richer model prices — not every model bills tokens.
--
-- Chat models bill per million tokens, but image models bill per image (or
-- image tokens), video models per second of OUTPUT (or tokens), and some
-- gateways per request. `billing_unit` says which; `unit_price_usd` carries the
-- per-image / per-second / per-request figure and `tier_prices_json` optional
-- per-resolution overrides of it (`{"1080p":0.4}`). The token columns from 014
-- stay and are simply 0 for a non-token model.
--
-- `source` records where the figure came from (manual / official / sync) and
-- `source_note` which catalog entry or address, so an admin looking at a price
-- months later can tell a contract price from a reference one.
ALTER TABLE one_model_prices ADD COLUMN billing_unit TEXT NOT NULL DEFAULT 'token';
ALTER TABLE one_model_prices ADD COLUMN unit_price_usd REAL NOT NULL DEFAULT 0;
ALTER TABLE one_model_prices ADD COLUMN tier_prices_json TEXT;
ALTER TABLE one_model_prices ADD COLUMN source TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE one_model_prices ADD COLUMN source_note TEXT;
