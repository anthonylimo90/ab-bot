'use client';

import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Plus, Building2, RefreshCw } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { WorkspaceTable } from '@/components/admin/WorkspaceTable';
import { CreateWorkspaceDialog } from '@/components/admin/CreateWorkspaceDialog';
import api from '@/lib/api';

export default function AdminWorkspacesPage() {
  const [createDialogOpen, setCreateDialogOpen] = useState(false);

  const { data: workspaces, isLoading, error, refetch } = useQuery({
    queryKey: ['admin', 'workspaces'],
    queryFn: () => api.adminListWorkspaces(),
  });

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Workspaces</h1>
          <p className="text-muted-foreground">
            Manage all workspaces in the platform
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="icon" onClick={() => refetch()}>
            <RefreshCw className="h-4 w-4" />
          </Button>
          <Button onClick={() => setCreateDialogOpen(true)}>
            <Plus className="mr-2 h-4 w-4" />
            Create Workspace
          </Button>
        </div>
      </div>

      {/* Stats Cards */}
      <div className="grid gap-4 md:grid-cols-3">
        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">Total Workspaces</CardTitle>
            <Building2 className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{workspaces?.length ?? 0}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">Manual Mode</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {workspaces?.filter((w) => w.setup_mode === 'manual').length ?? 0}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">Automatic Mode</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {workspaces?.filter((w) => w.setup_mode === 'automatic').length ?? 0}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Workspaces Table */}
      <Card>
        <CardHeader>
          <CardTitle>All Workspaces</CardTitle>
          <CardDescription>
            A list of all workspaces and their owners
          </CardDescription>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <div className="flex items-center justify-center py-12">
              <div className="h-8 w-8 animate-spin rounded-full border-4 border-primary border-t-transparent" />
            </div>
          ) : error ? (
            <div className="text-center py-12">
              <p className="text-destructive">Failed to load workspaces</p>
              <Button variant="outline" className="mt-4" onClick={() => refetch()}>
                Try Again
              </Button>
            </div>
          ) : (
            <WorkspaceTable workspaces={workspaces ?? []} />
          )}
        </CardContent>
      </Card>

      {/* Create Workspace Dialog */}
      <CreateWorkspaceDialog open={createDialogOpen} onOpenChange={setCreateDialogOpen} />
    </div>
  );
}
