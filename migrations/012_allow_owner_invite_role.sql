-- Allow 'owner' role in workspace invites for admin-created workspaces
-- where the owner doesn't have an account yet

ALTER TABLE workspace_invites
DROP CONSTRAINT IF EXISTS valid_invite_role;

ALTER TABLE workspace_invites
ADD CONSTRAINT valid_invite_role CHECK (role IN ('owner', 'admin', 'member', 'viewer'));
