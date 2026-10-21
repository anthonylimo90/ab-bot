-- Unknown legacy provenance must be reconciled before submitting another order.
ALTER TABLE positions ADD COLUMN execution_mode TEXT NOT NULL DEFAULT 'unknown'
    CHECK (execution_mode IN ('unknown','paper','live'));
ALTER TABLE positions ADD COLUMN wallet_address TEXT;
ALTER TABLE positions ADD COLUMN version BIGINT NOT NULL DEFAULT 0;
ALTER TABLE positions ADD CONSTRAINT live_position_wallet CHECK
    (execution_mode <> 'live' OR wallet_address IS NOT NULL);
