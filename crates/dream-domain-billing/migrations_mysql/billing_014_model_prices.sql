-- one-billing 014: administrator-entered chat token prices, one row per model
-- (MySQL port). See the SQLite file for why prices are keyed by model name
-- rather than stored on a channel.
CREATE TABLE IF NOT EXISTS one_model_prices (
    enterprise_id            VARCHAR(255) NOT NULL,
    model                    VARCHAR(255) NOT NULL,
    input_usd_per_mtok       DOUBLE       NOT NULL,
    output_usd_per_mtok      DOUBLE       NOT NULL,
    cache_read_usd_per_mtok  DOUBLE       NULL,
    cache_write_usd_per_mtok DOUBLE       NULL,
    updated_by               VARCHAR(255) NOT NULL,
    updated_at               BIGINT       NOT NULL,
    PRIMARY KEY (enterprise_id, model)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
