-- Migration: Add cleanup for expired auth challenges
-- Creates a function and trigger to automatically delete expired challenges

-- Function to delete expired challenges (older than 10 minutes)
CREATE OR REPLACE FUNCTION cleanup_expired_auth_challenges()
RETURNS INTEGER AS $$
DECLARE
    deleted_count INTEGER;
BEGIN
    DELETE FROM auth_challenges
    WHERE expires_at < NOW() - INTERVAL '10 minutes'
       OR used_at IS NOT NULL;
    GET DIAGNOSTICS deleted_count = ROW_COUNT;
    RETURN deleted_count;
END;
$$ LANGUAGE plpgsql;

-- Create index for efficient cleanup queries
CREATE INDEX IF NOT EXISTS idx_auth_challenges_expires
ON auth_challenges(expires_at)
WHERE used_at IS NULL;

-- Schedule cleanup to run periodically via application
-- (The application should call SELECT cleanup_expired_auth_challenges() periodically)

COMMENT ON FUNCTION cleanup_expired_auth_challenges() IS
'Deletes expired or used auth challenges. Call periodically via application or pg_cron.';
