-- Include pending reservations and actual held inventory in every lifecycle state.
-- Fully exited history contributes zero and does not need an index entry.
CREATE INDEX idx_positions_exposure_candidates
    ON positions(state,held_yes_qty,held_no_qty)
    INCLUDE(quantity,yes_entry_price,no_entry_price)
    WHERE state=0 OR held_yes_qty>0 OR held_no_qty>0;
