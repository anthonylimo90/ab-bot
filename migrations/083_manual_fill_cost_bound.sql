-- Cost manual buy fills of unknown price at the order's limit, not at $1.
--
-- An exchange snapshot reports how much of an order filled but not at what
-- price, so avg_fill_price stays NULL for those fills and the trigger from
-- 077 reserved the full $1 per share. A buy limit order never fills above its
-- limit (a resting order fills at it, a crossing one at the book or better),
-- so the limit is still a safe upper bound and far closer. Orders without a
-- price (market orders) keep the $1 bound. Only new fills are affected.

CREATE OR REPLACE FUNCTION account_manual_order_fill() RETURNS trigger LANGUAGE plpgsql AS $$
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
        -- Unknown execution prices are bounded by the limit (or $1 without one),
        -- never a guessed fill.
        fill_cost := CASE WHEN NEW.avg_fill_price IS NOT NULL AND (OLD.filled_quantity=0 OR OLD.avg_fill_price IS NOT NULL)
            THEN GREATEST(NEW.filled_quantity*NEW.avg_fill_price-OLD.filled_quantity*COALESCE(OLD.avg_fill_price,0),0)
            ELSE delta*LEAST(COALESCE(NEW.price,1),1) END;
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
