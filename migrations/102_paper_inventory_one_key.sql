-- Paper manual orders hold nothing in any wallet: they are recorded without
-- one, and their inventory is kept under the single key 'unknown' (as a
-- paper-only process always kept it). Paper orders placed while a trading
-- signer was loaded (live trading switched off at runtime) carried its
-- address, splitting paper inventory across two keys so a sale could not see
-- shares bought under the other. Merge them.
INSERT INTO manual_inventory (execution_mode, wallet_address, market_id, outcome, quantity, cost_basis)
SELECT 'paper', 'unknown', market_id, outcome, SUM(quantity), SUM(cost_basis)
FROM manual_inventory
WHERE execution_mode = 'paper' AND wallet_address <> 'unknown'
GROUP BY market_id, outcome
ON CONFLICT (execution_mode, wallet_address, market_id, outcome) DO UPDATE SET
    quantity = manual_inventory.quantity + EXCLUDED.quantity,
    cost_basis = manual_inventory.cost_basis + EXCLUDED.cost_basis;

DELETE FROM manual_inventory WHERE execution_mode = 'paper' AND wallet_address <> 'unknown';

-- The fill trigger acts only on a change of filled_quantity.
UPDATE orders SET wallet_address = NULL
WHERE execution_mode = 'paper' AND wallet_address IS NOT NULL;
