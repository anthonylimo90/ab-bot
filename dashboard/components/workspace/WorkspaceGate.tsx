'use client';

import { useRouter } from 'next/navigation';
import { Building2, LogOut, Loader2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { useAuthStore } from '@/stores/auth-store';
import { useWorkspaceContext } from '@/providers/WorkspaceProvider';

interface WorkspaceGateProps {
  children: React.ReactNode;
}

export function WorkspaceGate({ children }: WorkspaceGateProps) {
  const router = useRouter();
  const { logout } = useAuthStore();
  const { currentWorkspace } = useWorkspaceStore();
  const { isInitializing } = useWorkspaceContext();

  const handleLogout = () => {
    logout();
    router.push('/login');
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

  if (!currentWorkspace) {
    return (
      <div className="flex items-center justify-center min-h-screen bg-background p-4">
        <Card className="w-full max-w-md">
          <CardHeader className="text-center">
            <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-muted mx-auto mb-4">
              <Building2 className="h-6 w-6 text-muted-foreground" />
            </div>
            <CardTitle>No Trading Workspace Found</CardTitle>
            <CardDescription>
              Your account does not currently have access to the canonical trading workspace. Ask your administrator for an invite link.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="rounded-lg border bg-muted/30 p-4 space-y-2">
              <h4 className="text-sm font-medium">What to do next:</h4>
              <ul className="text-sm text-muted-foreground space-y-1">
                <li>1. Contact your team administrator</li>
                <li>2. Request access to the trading workspace</li>
                <li>3. Accept the invite and reload the dashboard</li>
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

  // Workspace is set - render children
  return <>{children}</>;
}
