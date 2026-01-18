'use client';

import { useWorkspaceStore } from '@/stores/workspace-store';
import { ManualDashboard } from '@/components/manual/ManualDashboard';
import { AutomaticDashboard } from '@/components/automatic/AutomaticDashboard';
import { Skeleton } from '@/components/ui/skeleton';
import { Card, CardContent } from '@/components/ui/card';

export default function DashboardPage() {
  const { currentWorkspace, isLoading, _hasHydrated } = useWorkspaceStore();

  // Wait for hydration
  if (!_hasHydrated) {
    return <DashboardSkeleton />;
  }

  // Show loading state while fetching workspace
  if (isLoading) {
    return <DashboardSkeleton />;
  }

  // No workspace selected - show manual dashboard as default
  if (!currentWorkspace) {
    return <ManualDashboard />;
  }

  // Route based on setup_mode
  if (currentWorkspace.setup_mode === 'automatic') {
    return <AutomaticDashboard />;
  }

  return <ManualDashboard />;
}

function DashboardSkeleton() {
  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <Skeleton className="h-9 w-40 mb-2" />
          <Skeleton className="h-4 w-56" />
        </div>
        <Skeleton className="h-10 w-36" />
      </div>

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <Card key={i}>
            <CardContent className="pt-6">
              <Skeleton className="h-4 w-24 mb-2" />
              <Skeleton className="h-8 w-32 mb-1" />
              <Skeleton className="h-3 w-20" />
            </CardContent>
          </Card>
        ))}
      </div>

      <div className="grid gap-6 lg:grid-cols-3">
        <Card className="lg:col-span-2">
          <CardContent className="pt-6">
            <Skeleton className="h-[300px] w-full" />
          </CardContent>
        </Card>
        <Card>
          <CardContent className="pt-6">
            <Skeleton className="h-[300px] w-full" />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
