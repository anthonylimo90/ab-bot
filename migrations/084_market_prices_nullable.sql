-- A market price (or end date) the catalog source did not report is unknown,
-- not 0.50 (or "a year from now").
--
-- markets.yes_price/no_price were NOT NULL DEFAULT 0.5 and the catalog wrote
-- 0.50 for any outcome without a price (and for every named-outcome market).
-- Position marks and the account ledger read these columns as real prices,
-- so an unknown price must be NULL: consumers then fall back to cost and
-- report the leg as unpriced. The next catalog refresh rewrites every active
-- market's prices.

ALTER TABLE markets ALTER COLUMN yes_price DROP DEFAULT;
ALTER TABLE markets ALTER COLUMN yes_price DROP NOT NULL;
ALTER TABLE markets ALTER COLUMN no_price DROP DEFAULT;
ALTER TABLE markets ALTER COLUMN no_price DROP NOT NULL;
-- An unknown end date was stored as "now + 365 days", a different value on
-- every refresh.
ALTER TABLE markets ALTER COLUMN end_date DROP NOT NULL;
