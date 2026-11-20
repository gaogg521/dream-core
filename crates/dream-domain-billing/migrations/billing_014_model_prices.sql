-- one-billing 014: administrator-entered chat token prices, one row per model.
--
-- Keyed by model NAME, not by channel. A price is what the vendor bills for a
-- model, so taking a model off a channel and putting it back, or listing it
-- on a second channel, must neither lose the price nor fork it into two. The
-- admin console's model price table is the one place these are edited, and
-- every costing path (model proxy, server-side turns, client usage reports)
-- reads them from here before falling back to the built-in rate table.
--
-- USD per million tokens. Input and output are both required: a half-entered
-- price would cost the other half of every call at zero. The cache columns are
-- optional and default from the input price at costing time (hits at 10%,
-- writes at 100%).
CREATE TABLE IF NOT EXISTS one_model_prices (
    enterprise_id            TEXT    NOT NULL,
    model                    TEXT    NOT NULL,
    input_usd_per_mtok       REAL    NOT NULL,
    output_usd_per_mtok      REAL    NOT NULL,
    cache_read_usd_per_mtok  REAL,
    cache_write_usd_per_mtok REAL,
    updated_by               TEXT    NOT NULL,
    updated_at               INTEGER NOT NULL,
    PRIMARY KEY (enterprise_id, model)
);
