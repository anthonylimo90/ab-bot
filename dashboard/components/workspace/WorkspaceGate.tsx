'use client';

import { useRouter } from 'next/navigation';
import { Building2, LogOut, Check, Loader2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { useAuthStore } from '@/stores/auth-store';
import { useWorkspaceContext } from '@/providers/WorkspaceProvider';
import { cn } from '@/lib/utils';

interface WorkspaceGateProps {
  children: React.ReactNode;
}

export function WorkspaceGate({ children }: WorkspaceGateProps) {
  const router = useRouter();
  const { logout } = useAuthStore();
  const {
    workspaces,
    currentWorkspace,
    isLoading,
    switchWorkspace,
  } = useWorkspaceStore();
  const { isInitializing, needsWorkspaceSelection } = useWorkspaceContext();

  const handleLogout = () => {
    logout();
    router.push('/login');
  };

  const handleSelectWorkspace = async (workspaceId: string) => {
    try {
      await switchWorkspace(workspaceId);
      router.refresh();
    } catch {
      // Error handled in store
    }
  };

  // Show loading state while initializing
  if (isInitializing) {
    return (
      <div className="flex items-center justify-center min-h-screen bg-background">
        <div className="flex flex-col items-center gap-4">
          <Loader2 className="h-8 w-8 animate-spin text-primary" />
          <p className="text-sm text-muted-foreground">Loading workspace...</p>
        </div>
      </div>
    );
  }

  // No workspaces - show recovery screen
  if (!currentWorkspace && workspaces.length === 0) {
    return (
      <div className="flex items-center justify-center min-h-screen bg-background p-4">
        <Card className="w-full max-w-md">
          <CardHeader className="text-center">
            <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-muted mx-auto mb-4">
              <Building2 className="h-6 w-6 text-muted-foreground" />
            </div>
            <CardTitle>No Workspace Found</CardTitle>
            <CardDescription>
              You don&apos;t have access to any workspace yet. Ask your administrator for an invite link.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="rounded-lg border bg-muted/30 p-4 space-y-2">
              <h4 className="text-sm font-medium">What to do next:</h4>
              <ul className="text-sm text-muted-foreground space-y-1">
                <li>1. Contact your team administrator</li>
                <li>2. Request an invite link to their workspace</li>
                <li>3. Click the link in your email to join</li>
              </ul>
            </div>
            <Button
              variant="outline"
              className="w-full"
              onClick={handleLogout}
            >
              <LogOut className="mr-2 h-4 w-4" />
              Sign Out
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  // Multiple workspaces but none selected - show picker
  if (needsWorkspaceSelection) {
    return (
      <div className="flex items-center justify-center min-h-screen bg-background p-4">
        <Card className="w-full max-w-lg">
          <CardHeader className="text-center">
            <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-primary text-primary-foreground font-bold text-xl mx-auto mb-4">
              AB
            </div>
            <CardTitle>Select a Workspace</CardTitle>
            <CardDescription>
              Choose a workspace to continue to the dashboard.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {workspaces.map((workspace) => (
              <button
                key={workspace.id}
                onClick={() => handleSelectWorkspace(workspace.id)}
                disabled={isLoading}
                className={cn(
                  'w-full flex items-center justify-between p-4 rounded-lg border transition-all',
                  'hover:border-primary hover:bg-primary/5',
                  'disabled:opacity-50 disabled:cursor-not-allowed',
                  currentWorkspace?.id === workspace.id && 'border-primary bg-primary/5'
                )}
              >
                <div className="flex items-center gap-3">
                  <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-muted">
                    <Building2 className="h-5 w-5 text-muted-foreground" />
                  </div>
                  <div className="text-left">
                    <p className="font-medium">{workspace.name}</p>
                    <p className="text-sm text-muted-foreground capitalize">
                      {workspace.my_role} role
                    </p>
                  </div>
                </div>
                {currentWorkspace?.id === workspace.id ? (
                  <Check className="h-5 w-5 text-primary" />
                ) : isLoading ? (
                  <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
                ) : null}
              </button>
            ))}
          </CardContent>
        </Card>
      </div>
    );
  }

  // Workspace is set - render children
  return <>{children}</>;
}
