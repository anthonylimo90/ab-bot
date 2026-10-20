CREATE TABLE position_order_links (
    id UUID PRIMARY KEY,
    position_id UUID NOT NULL REFERENCES positions(id),
    leg TEXT NOT NULL CHECK (leg IN ('yes','no')),
    side TEXT NOT NULL CHECK (side IN ('buy','sell')),
    requested_quantity NUMERIC NOT NULL CHECK (requested_quantity > 0),
    applied_quantity NUMERIC NOT NULL DEFAULT 0,
    settled BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX position_order_links_pending ON position_order_links(position_id) WHERE NOT settled;
