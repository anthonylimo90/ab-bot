-- Migration: Add WalletConnect project ID to workspaces
-- This allows each workspace owner to configure their own WalletConnect project ID

ALTER TABLE workspaces
ADD COLUMN IF NOT EXISTS walletconnect_project_id VARCHAR(64);

-- Add a comment explaining the field
COMMENT ON COLUMN workspaces.walletconnect_project_id IS 'WalletConnect Cloud project ID for wallet connection';
