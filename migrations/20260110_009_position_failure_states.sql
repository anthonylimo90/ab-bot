-- Migration: Add failure states and recovery fields to positions table
-- This enables proper error handling and recovery for position lifecycle

-- Add new columns for failure tracking
ALTER TABLE positions
ADD COLUMN IF NOT EXISTS failure_reason TEXT,
ADD COLUMN IF NOT EXISTS retry_count INTEGER DEFAULT 0,
ADD COLUMN IF NOT EXISTS last_updated TIMESTAMPTZ DEFAULT NOW();

-- Update last_updated for existing positions to their entry_timestamp
UPDATE positions
SET last_updated = entry_timestamp
WHERE last_updated IS NULL;

-- Create index for finding positions needing recovery
CREATE INDEX IF NOT EXISTS idx_positions_recovery
ON positions (state)
WHERE state IN (6, 7);

-- Create index for stale position detection
CREATE INDEX IF NOT EXISTS idx_positions_last_updated
ON positions (last_updated)
WHERE state NOT IN (4, 5);

-- Add comment documenting the new state values
COMMENT ON COLUMN positions.state IS 'Position state: 0=Pending, 1=Open, 2=ExitReady, 3=Closing, 4=Closed, 5=EntryFailed, 6=ExitFailed, 7=Stalled';
COMMENT ON COLUMN positions.failure_reason IS 'JSON-serialized FailureReason when position is in failed/stalled state';
COMMENT ON COLUMN positions.retry_count IS 'Number of retry attempts made for failed positions';
COMMENT ON COLUMN positions.last_updated IS 'Last time this position was updated, used for stale detection';
