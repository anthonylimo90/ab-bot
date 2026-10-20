ALTER TABLE orders ADD COLUMN IF NOT EXISTS exchange_order_id TEXT;
-- Historical rows have unknown execution provenance; never cancel them locally
-- while claiming that an exchange order was cancelled.
ALTER TABLE orders ADD COLUMN IF NOT EXISTS execution_mode TEXT NOT NULL DEFAULT 'unknown';
CREATE INDEX IF NOT EXISTS idx_orders_exchange_id ON orders(exchange_order_id)
    WHERE exchange_order_id IS NOT NULL;
