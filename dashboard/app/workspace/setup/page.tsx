'use client';

import { useEffect } from 'react';
import { useRouter } from 'next/navigation';
import { useQuery } from '@tanstack/react-query';
import { SetupWizard } from '@/components/setup/SetupWizard';
import { useAuthStore } from '@/stores/auth-store';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import api from '@/lib/api';
import { Loader2, Building2, LogOut, Mail, HelpCircle } from 'lucide-react';

export default function WorkspaceSetupPage() {
  const router = useRouter();
  const { isAuthenticated, isLoading: authLoading, logout, user } = useAuthStore();

  const { data: status, isLoading: statusLoading, error } = useQuery({
    queryKey: ['onboarding', 'status'],
    queryFn: () => api.getOnboardingStatus(),
    enabled: isAuthenticated,
    retry: 1,
  });

  // Redirect if already completed onboarding
  useEffect(() => {
    if (status?.onboarding_completed) {
      router.push('/');
    }
  }, [status, router]);

  const handleLogout = () => {
    logout();
    router.push('/login');
  };

  const isLoading = authLoading || statusLoading;

  if (isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <div className="flex flex-col items-center gap-4">
          <Loader2 className="h-8 w-8 animate-spin text-primary" />
          <p className="text-sm text-muted-foreground">Loading workspace...</p>
        </div>
      </div>
    );
  }

  // No workspace found - show recovery options
  if (!status || error) {
    return (
      <div className="min-h-screen flex items-center justify-center p-4">
        <Card className="w-full max-w-md">
          <CardHeader className="text-center">
            <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-muted mx-auto mb-4">
              <Building2 className="h-6 w-6 text-muted-foreground" />
            </div>
            <CardTitle>No Workspace Found</CardTitle>
            <CardDescription>
              You don&apos;t have access to any workspace yet.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            {user && (
              <div className="rounded-lg border bg-muted/30 p-3 text-center">
                <p className="text-sm text-muted-foreground">Signed in as</p>
                <p className="font-medium">{user.email}</p>
              </div>
            )}

            <div className="space-y-3">
              <h4 className="text-sm font-medium">How to get access:</h4>
              <ul className="space-y-2 text-sm text-muted-foreground">
                <li className="flex items-start gap-2">
                  <Mail className="h-4 w-4 mt-0.5 shrink-0" />
                  <span>Ask your team administrator to send you an invite link</span>
                </li>
                <li className="flex items-start gap-2">
                  <HelpCircle className="h-4 w-4 mt-0.5 shrink-0" />
                  <span>Check your email for any pending workspace invitations</span>
                </li>
              </ul>
            </div>

            <div className="pt-4 border-t space-y-2">
              <Button
                variant="outline"
                className="w-full"
                onClick={handleLogout}
              >
                <LogOut className="mr-2 h-4 w-4" />
                Sign Out
              </Button>
              <p className="text-xs text-center text-muted-foreground">
                Sign out to use a different account
              </p>
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-background py-8 px-4">
      <div className="mb-8 text-center">
        <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-primary text-primary-foreground font-bold text-xl mx-auto mb-4">
          AB
        </div>
        <h1 className="text-3xl font-bold">Welcome to AB-Bot</h1>
        <p className="text-muted-foreground mt-2">
          Let&apos;s set up your workspace: {status.workspace_name}
        </p>
      </div>
      <SetupWizard initialStatus={status} />
    </div>
  );
}
