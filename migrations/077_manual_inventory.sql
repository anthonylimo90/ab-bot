ALTER TABLE orders ADD COLUMN wallet_address TEXT;
CREATE TABLE manual_inventory (
    execution_mode TEXT NOT NULL,
    wallet_address TEXT NOT NULL,
    market_id TEXT NOT NULL,
    outcome TEXT NOT NULL,
    quantity NUMERIC NOT NULL DEFAULT 0 CHECK (quantity >= 0),
    cost_basis NUMERIC NOT NULL DEFAULT 0 CHECK (cost_basis >= 0),
    PRIMARY KEY (execution_mode, wallet_address, market_id, outcome)
);
-- Historical sales lack lot allocation. Preserve a conservative gross purchase
-- basis for any remaining shares until an authoritative inventory reconciliation.
INSERT INTO manual_inventory
SELECT execution_mode, COALESCE(LOWER(wallet_address),'unknown'), market_id, outcome,
    GREATEST(SUM(CASE WHEN side='buy' THEN filled_quantity ELSE -filled_quantity END),0),
    CASE WHEN SUM(CASE WHEN side='buy' THEN filled_quantity ELSE -filled_quantity END)>0
        THEN SUM(CASE WHEN side='buy' THEN filled_quantity*COALESCE(avg_fill_price,price,1) ELSE 0 END) ELSE 0 END
FROM orders GROUP BY execution_mode, COALESCE(LOWER(wallet_address),'unknown'), market_id, outcome;

CREATE FUNCTION account_manual_order_fill() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE delta NUMERIC; prior_quantity NUMERIC; prior_cost NUMERIC; fill_cost NUMERIC;
BEGIN
    IF NEW.filled_quantity < OLD.filled_quantity THEN
        RAISE EXCEPTION 'Confirmed fills cannot decrease';
    END IF;
    delta := NEW.filled_quantity - OLD.filled_quantity;
    IF delta = 0 THEN RETURN NEW; END IF;
    INSERT INTO manual_inventory (execution_mode,wallet_address,market_id,outcome)
        VALUES (NEW.execution_mode,COALESCE(LOWER(NEW.wallet_address),'unknown'),NEW.market_id,NEW.outcome)
        ON CONFLICT DO NOTHING;
    SELECT quantity,cost_basis INTO prior_quantity,prior_cost FROM manual_inventory
        WHERE execution_mode=NEW.execution_mode AND wallet_address=COALESCE(LOWER(NEW.wallet_address),'unknown')
          AND market_id=NEW.market_id AND outcome=NEW.outcome FOR UPDATE;
    IF NEW.side='buy' THEN
        -- Missing execution prices reserve the full $1 per share, never a guessed fill.
        fill_cost := CASE WHEN NEW.avg_fill_price IS NOT NULL AND (OLD.filled_quantity=0 OR OLD.avg_fill_price IS NOT NULL)
            THEN GREATEST(NEW.filled_quantity*NEW.avg_fill_price-OLD.filled_quantity*COALESCE(OLD.avg_fill_price,0),0)
            ELSE delta END;
        UPDATE manual_inventory SET quantity=prior_quantity+delta,cost_basis=prior_cost+fill_cost
          WHERE execution_mode=NEW.execution_mode AND wallet_address=COALESCE(LOWER(NEW.wallet_address),'unknown')
            AND market_id=NEW.market_id AND outcome=NEW.outcome;
    ELSE
        IF delta > prior_quantity THEN RAISE EXCEPTION 'Manual sale exceeds tracked inventory'; END IF;
        UPDATE manual_inventory SET quantity=prior_quantity-delta,
            cost_basis=CASE WHEN prior_quantity=0 THEN 0 ELSE prior_cost*(prior_quantity-delta)/prior_quantity END
          WHERE execution_mode=NEW.execution_mode AND wallet_address=COALESCE(LOWER(NEW.wallet_address),'unknown')
            AND market_id=NEW.market_id AND outcome=NEW.outcome;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER manual_order_fill AFTER UPDATE OF filled_quantity ON orders
    FOR EACH ROW EXECUTE FUNCTION account_manual_order_fill();
