-- Track when optimizer last ran for each workspace
ALTER TABLE workspaces
ADD COLUMN IF NOT EXISTS last_optimization_at TIMESTAMPTZ;

-- Index for efficient filtering
CREATE INDEX IF NOT EXISTS idx_workspaces_last_optimization
ON workspaces(last_optimization_at)
WHERE auto_optimize_enabled = true;
