-- Track the concrete CLOB token ID for copy-trade positions.
-- `positions.outcome` remains semantic ('yes'/'no') for UI/API compatibility.
ALTER TABLE positions
ADD COLUMN IF NOT EXISTS source_token_id VARCHAR(255);

CREATE INDEX IF NOT EXISTS idx_positions_source_token_id
ON positions(source_token_id)
WHERE source_token_id IS NOT NULL;
