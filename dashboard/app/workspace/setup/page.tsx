'use client';

import { useEffect } from 'react';
import { useRouter } from 'next/navigation';
import { useQuery } from '@tanstack/react-query';
import { SetupWizard } from '@/components/setup/SetupWizard';
import { useAuthStore } from '@/stores/auth-store';
import api from '@/lib/api';
import { Loader2 } from 'lucide-react';

export default function WorkspaceSetupPage() {
  const router = useRouter();
  const { isAuthenticated, isLoading: authLoading } = useAuthStore();

  const { data: status, isLoading: statusLoading } = useQuery({
    queryKey: ['onboarding', 'status'],
    queryFn: () => api.getOnboardingStatus(),
    enabled: isAuthenticated,
  });

  // Redirect if already completed onboarding
  useEffect(() => {
    if (status?.onboarding_completed) {
      router.push('/');
    }
  }, [status, router]);

  const isLoading = authLoading || statusLoading;

  if (isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <div className="flex flex-col items-center gap-4">
          <Loader2 className="h-8 w-8 animate-spin text-primary" />
          <p className="text-sm text-muted-foreground">Loading...</p>
        </div>
      </div>
    );
  }

  if (!status) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <div className="text-center space-y-4">
          <p className="text-muted-foreground">No workspace found</p>
          <p className="text-sm text-muted-foreground">
            Please contact an administrator to be invited to a workspace.
          </p>
        </div>
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
