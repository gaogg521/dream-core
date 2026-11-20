-- one-billing 015: richer model prices (MySQL port). See the SQLite file for
-- why a price carries a billing unit, a per-unit figure, tiers and a source.
ALTER TABLE one_model_prices ADD COLUMN billing_unit VARCHAR(16) NOT NULL DEFAULT 'token';
ALTER TABLE one_model_prices ADD COLUMN unit_price_usd DOUBLE NOT NULL DEFAULT 0;
ALTER TABLE one_model_prices ADD COLUMN tier_prices_json TEXT NULL;
ALTER TABLE one_model_prices ADD COLUMN source VARCHAR(16) NOT NULL DEFAULT 'manual';
ALTER TABLE one_model_prices ADD COLUMN source_note VARCHAR(512) NULL;
