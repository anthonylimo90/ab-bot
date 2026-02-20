-- Add exit handler toggle to workspaces
ALTER TABLE workspaces ADD COLUMN IF NOT EXISTS exit_handler_enabled BOOLEAN DEFAULT FALSE;
