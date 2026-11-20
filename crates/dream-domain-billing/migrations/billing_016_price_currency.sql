-- one-billing 016: model prices can be entered in RMB.
--
-- The ledger, spend caps and reports are USD-micros, so a price keeps the currency
-- the admin entered it in (a vendor that bills in RMB is the source of truth in RMB)
-- and is converted to USD at a FIXED rate (`CNY_PER_USD`, 7) when a call is costed.
-- There is deliberately no configurable exchange rate.
--
-- The `*_usd_*` column names predate this and now mean "in this row's currency".
ALTER TABLE one_model_prices ADD COLUMN currency TEXT NOT NULL DEFAULT 'USD';
