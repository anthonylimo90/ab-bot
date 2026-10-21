-- Password reset redemption looks tokens up by their SHA-256 hash:
--   WHERE token_hash = $1 AND expires_at > NOW() AND used_at IS NULL
-- but only user_id and expires_at were indexed, so every redemption attempt (an
-- unauthenticated endpoint) scanned the whole table. Index the hash of the
-- tokens that can still be redeemed. Deliberately not UNIQUE: a duplicate in
-- legacy data must never be able to fail this migration.

CREATE INDEX IF NOT EXISTS idx_password_reset_tokens_token_hash_unused
    ON password_reset_tokens (token_hash)
    WHERE used_at IS NULL;
