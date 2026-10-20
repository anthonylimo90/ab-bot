-- Intentionally preserve failed/stalled positions. Age does not establish that
-- an entry failed, an exit completed, or on-chain inventory disappeared.
-- This replaces the unsafe cleanup that also mixed TEXT failure_reason with an
-- integer fallback. Closure belongs to exchange/on-chain reconciliation.
SELECT 1;
