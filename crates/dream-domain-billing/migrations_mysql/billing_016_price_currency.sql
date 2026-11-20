-- one-billing 016: model prices can be entered in RMB (MySQL port). See the SQLite
-- file for why a price keeps its currency and the rate is fixed.
ALTER TABLE one_model_prices ADD COLUMN currency VARCHAR(8) NOT NULL DEFAULT 'USD';
