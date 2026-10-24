-- Manual shares held to a market's resolution realize its payout less their
-- cost without any sale: record that result too, once per holding, so the
-- circuit breaker counts it like a sale's. The exit handler writes these
-- rows when it finds the market resolved with a named winner and no manual
-- order still open in it (a sale booked later would count its shares twice).
ALTER TABLE manual_realized_results
    ALTER COLUMN order_id DROP NOT NULL,
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'sale' CHECK (kind IN ('sale', 'resolution')),
    ADD COLUMN wallet_address TEXT,
    ADD CONSTRAINT manual_realized_results_kind_source CHECK (
        (kind = 'sale' AND order_id IS NOT NULL)
        OR (kind = 'resolution' AND order_id IS NULL AND wallet_address IS NOT NULL)
    );

CREATE UNIQUE INDEX idx_manual_realized_results_one_resolution
    ON manual_realized_results (wallet_address, market_id, outcome)
    WHERE kind = 'resolution';
