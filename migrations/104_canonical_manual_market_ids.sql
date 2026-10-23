-- Manual orders now store a hex condition id as the catalog does: lowercase.
-- Rows stored before kept the client's casing, so their inventory would no
-- longer match new orders (or exits and orphan checks, which match by id).
-- Merge them under the lowercase id.
INSERT INTO manual_inventory (execution_mode, wallet_address, market_id, outcome, quantity, cost_basis)
SELECT execution_mode, wallet_address, LOWER(market_id), outcome, SUM(quantity), SUM(cost_basis)
FROM manual_inventory
WHERE market_id ~ '^0[xX][0-9a-fA-F]+$' AND market_id <> LOWER(market_id)
GROUP BY execution_mode, wallet_address, LOWER(market_id), outcome
ON CONFLICT (execution_mode, wallet_address, market_id, outcome) DO UPDATE SET
    quantity = manual_inventory.quantity + EXCLUDED.quantity,
    cost_basis = manual_inventory.cost_basis + EXCLUDED.cost_basis;

DELETE FROM manual_inventory
WHERE market_id ~ '^0[xX][0-9a-fA-F]+$' AND market_id <> LOWER(market_id);

-- The fill trigger acts only on a change of filled_quantity.
UPDATE orders SET market_id = LOWER(market_id)
WHERE market_id ~ '^0[xX][0-9a-fA-F]+$' AND market_id <> LOWER(market_id);
