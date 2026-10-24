-- Manual holdings already held when resolution results started being
-- recorded (108). Some sit in markets that resolved long ago (the shares
-- redeemed or never sold): booking those results now would count weeks of
-- outcomes as today's loss and trip the circuit breaker. A holding listed
-- here whose market is found resolved on the first look is recorded as
-- seen, not counted; once its market is seen unresolved it leaves the list,
-- and its later resolution counts as any other.
CREATE TABLE manual_resolution_backlog (
    wallet_address TEXT NOT NULL,
    market_id TEXT NOT NULL,
    outcome TEXT NOT NULL,
    PRIMARY KEY (wallet_address, market_id, outcome)
);

INSERT INTO manual_resolution_backlog (wallet_address, market_id, outcome)
SELECT mi.wallet_address, mi.market_id, mi.outcome
FROM manual_inventory mi
WHERE mi.execution_mode = 'live' AND mi.quantity > 0
  AND NOT EXISTS (
      SELECT 1 FROM manual_realized_results r
      WHERE r.kind = 'resolution' AND r.wallet_address = mi.wallet_address
        AND r.market_id = mi.market_id AND r.outcome = mi.outcome)
ON CONFLICT DO NOTHING;
