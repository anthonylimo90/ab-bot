'use client';

import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { MoreHorizontal, Trash2, Eye } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import api from '@/lib/api';
import type { WorkspaceListItem } from '@/types/api';

interface WorkspaceTableProps {
  workspaces: WorkspaceListItem[];
}

export function WorkspaceTable({ workspaces }: WorkspaceTableProps) {
  const queryClient = useQueryClient();
  const [deleteWorkspace, setDeleteWorkspace] = useState<WorkspaceListItem | null>(null);

  const deleteMutation = useMutation({
    mutationFn: (workspaceId: string) => api.adminDeleteWorkspace(workspaceId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['admin', 'workspaces'] });
      setDeleteWorkspace(null);
    },
  });

  const formatDate = (dateStr: string) => {
    return new Date(dateStr).toLocaleDateString('en-US', {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
    });
  };

  if (workspaces.length === 0) {
    return (
      <div className="text-center py-12">
        <p className="text-muted-foreground">No workspaces found</p>
        <p className="text-sm text-muted-foreground mt-1">
          Create a workspace to get started
        </p>
      </div>
    );
  }

  return (
    <>
      <div className="overflow-x-auto rounded-md border">
        <table className="w-full min-w-[760px]">
          <thead>
            <tr className="border-b bg-muted/50">
              <th className="h-12 px-4 text-left align-middle font-medium text-muted-foreground">
                Name
              </th>
              <th className="h-12 px-4 text-left align-middle font-medium text-muted-foreground">
                Owner
              </th>
              <th className="h-12 px-4 text-left align-middle font-medium text-muted-foreground">
                Members
              </th>
              <th className="h-12 px-4 text-left align-middle font-medium text-muted-foreground">
                Mode
              </th>
              <th className="h-12 px-4 text-left align-middle font-medium text-muted-foreground">
                Created
              </th>
              <th className="h-12 px-4 text-right align-middle font-medium text-muted-foreground">
                Actions
              </th>
            </tr>
          </thead>
          <tbody>
            {workspaces.map((workspace) => (
              <tr key={workspace.id} className="border-b">
                <td className="p-4">
                  <div className="font-medium">{workspace.name}</div>
                  {workspace.description && (
                    <div className="text-sm text-muted-foreground truncate max-w-xs">
                      {workspace.description}
                    </div>
                  )}
                </td>
                <td className="p-4">
                  <span className="text-sm">{workspace.owner_email || '-'}</span>
                </td>
                <td className="p-4">
                  <span className="text-sm">{workspace.member_count}</span>
                </td>
                <td className="p-4">
                  <Badge variant={workspace.setup_mode === 'automatic' ? 'default' : 'secondary'}>
                    {workspace.setup_mode}
                  </Badge>
                </td>
                <td className="p-4">
                  <span className="text-sm text-muted-foreground">
                    {formatDate(workspace.created_at)}
                  </span>
                </td>
                <td className="p-4 text-right">
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button variant="ghost" size="icon">
                        <MoreHorizontal className="h-4 w-4" />
                        <span className="sr-only">Actions</span>
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem>
                        <Eye className="mr-2 h-4 w-4" />
                        View Details
                      </DropdownMenuItem>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem
                        className="text-destructive"
                        onClick={() => setDeleteWorkspace(workspace)}
                      >
                        <Trash2 className="mr-2 h-4 w-4" />
                        Delete
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* Delete Confirmation Dialog */}
      <AlertDialog open={!!deleteWorkspace} onOpenChange={() => setDeleteWorkspace(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete Workspace</AlertDialogTitle>
            <AlertDialogDescription>
              Are you sure you want to delete &quot;{deleteWorkspace?.name}&quot;? This action cannot
              be undone. All workspace data, members, and allocations will be permanently deleted.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteMutation.isPending}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() => deleteWorkspace && deleteMutation.mutate(deleteWorkspace.id)}
              disabled={deleteMutation.isPending}
            >
              {deleteMutation.isPending ? 'Deleting...' : 'Delete'}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
