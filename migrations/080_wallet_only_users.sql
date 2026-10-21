-- Wallet-only (SIWE) accounts have no email address and no password.
--
-- 002_trading_tables.sql declared users.email and users.password_hash NOT NULL,
-- so the first-time wallet sign-in INSERT (wallet_address only) always failed.
-- Both columns become nullable, and a CHECK keeps every account usable: it needs
-- either a full email/password credential pair or a linked wallet.
-- Existing rows all have email and password_hash, so the constraint holds for them.
-- (The UNIQUE constraint on email still applies; PostgreSQL allows many NULLs.)

ALTER TABLE users ALTER COLUMN email DROP NOT NULL;
ALTER TABLE users ALTER COLUMN password_hash DROP NOT NULL;

ALTER TABLE users DROP CONSTRAINT IF EXISTS users_credentials_present;
ALTER TABLE users ADD CONSTRAINT users_credentials_present CHECK (
    (email IS NOT NULL AND password_hash IS NOT NULL)
    OR wallet_address IS NOT NULL
);
