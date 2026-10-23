-- Realized results of live manual sales, for the circuit breaker.
--
-- Each confirmed fill of a live manual sell order realizes its proceeds less
-- the cost basis the sale releases from manual inventory. The fill trigger
-- writes one row per fill; the trading leader records each with the circuit
-- breaker and marks it (breaker_recorded_at) in the save that stores the
-- counters, as it does for position closes. Paper orders reach no breaker.
--
-- Proceeds of a fill whose price is not known are counted at the order's
-- limit (a sell never fills below it), or at nothing without one (a market
-- order): a loss is never understated. Fees are not deducted.

CREATE TABLE manual_realized_results (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    order_id UUID NOT NULL REFERENCES orders(id),
    market_id TEXT NOT NULL,
    outcome TEXT NOT NULL,
    quantity NUMERIC NOT NULL,
    proceeds NUMERIC NOT NULL,
    released_cost NUMERIC NOT NULL,
    realized_pnl NUMERIC NOT NULL,
    -- Whether the fill's price was reported (else bounded as above).
    priced BOOLEAN NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    breaker_recorded_at TIMESTAMPTZ
);

CREATE INDEX idx_manual_realized_results_unrecorded
    ON manual_realized_results (occurred_at)
    WHERE breaker_recorded_at IS NULL;

CREATE OR REPLACE FUNCTION account_manual_order_fill() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE delta NUMERIC; prior_quantity NUMERIC; prior_cost NUMERIC; fill_cost NUMERIC;
        priced BOOLEAN; remaining_cost NUMERIC;
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
    -- The fill's value is known when the average price covers every fill so far.
    priced := NEW.avg_fill_price IS NOT NULL AND (OLD.filled_quantity=0 OR OLD.avg_fill_price IS NOT NULL);
    IF NEW.side='buy' THEN
        -- Unknown execution prices are bounded by the limit (or $1 without one),
        -- never a guessed fill.
        fill_cost := CASE WHEN priced
            THEN GREATEST(NEW.filled_quantity*NEW.avg_fill_price-OLD.filled_quantity*COALESCE(OLD.avg_fill_price,0),0)
            ELSE delta*LEAST(COALESCE(NEW.price,1),1) END;
        UPDATE manual_inventory SET quantity=prior_quantity+delta,cost_basis=prior_cost+fill_cost
          WHERE execution_mode=NEW.execution_mode AND wallet_address=COALESCE(LOWER(NEW.wallet_address),'unknown')
            AND market_id=NEW.market_id AND outcome=NEW.outcome;
    ELSE
        IF delta > prior_quantity THEN RAISE EXCEPTION 'Manual sale exceeds tracked inventory'; END IF;
        remaining_cost := CASE WHEN prior_quantity=0 THEN 0 ELSE prior_cost*(prior_quantity-delta)/prior_quantity END;
        UPDATE manual_inventory SET quantity=prior_quantity-delta, cost_basis=remaining_cost
          WHERE execution_mode=NEW.execution_mode AND wallet_address=COALESCE(LOWER(NEW.wallet_address),'unknown')
            AND market_id=NEW.market_id AND outcome=NEW.outcome;
        IF NEW.execution_mode='live' THEN
            -- Unknown proceeds are bounded below by the limit (nothing without one).
            fill_cost := CASE WHEN priced
                THEN GREATEST(NEW.filled_quantity*NEW.avg_fill_price-OLD.filled_quantity*COALESCE(OLD.avg_fill_price,0),0)
                ELSE delta*LEAST(GREATEST(COALESCE(NEW.price,0),0),1) END;
            INSERT INTO manual_realized_results
                (order_id,market_id,outcome,quantity,proceeds,released_cost,realized_pnl,priced)
            VALUES (NEW.id,NEW.market_id,NEW.outcome,delta,fill_cost,prior_cost-remaining_cost,
                    fill_cost-(prior_cost-remaining_cost),priced);
        END IF;
    END IF;
    RETURN NEW;
END $$;
