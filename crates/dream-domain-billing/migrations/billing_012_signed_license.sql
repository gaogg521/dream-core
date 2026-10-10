-- Preserve the exact signed envelope. Legacy rows remain unverified until reactivation.
ALTER TABLE one_license_activation ADD COLUMN license_key TEXT;
