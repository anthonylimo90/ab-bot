'use client';

import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { MoreHorizontal, UserMinus, Shield, Eye, User } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
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
import type { WorkspaceMember, WorkspaceRole } from '@/types/api';
import { useAuthStore } from '@/stores/auth-store';

interface MemberListProps {
  workspaceId: string;
  members: WorkspaceMember[];
  currentUserRole?: WorkspaceRole;
}

export function MemberList({ workspaceId, members, currentUserRole }: MemberListProps) {
  const queryClient = useQueryClient();
  const { user: currentUser } = useAuthStore();
  const [removeMember, setRemoveMember] = useState<WorkspaceMember | null>(null);

  const canManageMembers = currentUserRole === 'owner' || currentUserRole === 'admin';

  const updateRoleMutation = useMutation({
    mutationFn: ({ memberId, role }: { memberId: string; role: WorkspaceRole }) =>
      api.updateMemberRole(workspaceId, memberId, role),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['workspace', workspaceId, 'members'] });
    },
  });

  const removeMutation = useMutation({
    mutationFn: (memberId: string) => api.removeMember(workspaceId, memberId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['workspace', workspaceId, 'members'] });
      setRemoveMember(null);
    },
  });

  const getRoleIcon = (role: WorkspaceRole) => {
    switch (role) {
      case 'owner':
        return <Shield className="h-4 w-4 text-yellow-500" />;
      case 'admin':
        return <Shield className="h-4 w-4 text-blue-500" />;
      case 'member':
        return <User className="h-4 w-4 text-gray-500" />;
      case 'viewer':
        return <Eye className="h-4 w-4 text-gray-400" />;
    }
  };

  const getRoleBadgeVariant = (
    role: WorkspaceRole
  ): 'default' | 'secondary' | 'warning' | 'outline' => {
    switch (role) {
      case 'owner':
        return 'warning';
      case 'admin':
        return 'default';
      case 'member':
        return 'secondary';
      case 'viewer':
        return 'outline';
    }
  };

  const formatDate = (dateStr: string) => {
    return new Date(dateStr).toLocaleDateString('en-US', {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
    });
  };

  if (members.length === 0) {
    return (
      <div className="text-center py-8 text-muted-foreground">
        No members yet
      </div>
    );
  }

  return (
    <>
      <div className="space-y-2">
        {members.map((member) => {
          const isCurrentUser = member.user_id === currentUser?.id;
          const isOwner = member.role === 'owner';
          const canModify = canManageMembers && !isOwner && !isCurrentUser;

          return (
            <div
              key={member.user_id}
              className="flex items-center justify-between p-4 rounded-lg border hover:bg-muted/50 transition-colors"
            >
              <div className="flex items-center gap-4">
                <div className="flex items-center justify-center h-10 w-10 rounded-full bg-muted">
                  {getRoleIcon(member.role)}
                </div>
                <div>
                  <div className="flex items-center gap-2">
                    <span className="font-medium">{member.email || 'Unknown'}</span>
                    <Badge variant={getRoleBadgeVariant(member.role)}>
                      {member.role}
                    </Badge>
                    {isCurrentUser && (
                      <Badge variant="success">You</Badge>
                    )}
                  </div>
                  <div className="text-sm text-muted-foreground">
                    {member.name || 'No name'} &middot; Joined {formatDate(member.joined_at)}
                  </div>
                </div>
              </div>
              {canModify && (
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button variant="ghost" size="icon">
                      <MoreHorizontal className="h-4 w-4" />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuLabel>Change Role</DropdownMenuLabel>
                    <DropdownMenuItem
                      onClick={() =>
                        updateRoleMutation.mutate({ memberId: member.user_id, role: 'admin' })
                      }
                      disabled={member.role === 'admin'}
                    >
                      Admin
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      onClick={() =>
                        updateRoleMutation.mutate({ memberId: member.user_id, role: 'member' })
                      }
                      disabled={member.role === 'member'}
                    >
                      Member
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      onClick={() =>
                        updateRoleMutation.mutate({ memberId: member.user_id, role: 'viewer' })
                      }
                      disabled={member.role === 'viewer'}
                    >
                      Viewer
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem
                      className="text-destructive"
                      onClick={() => setRemoveMember(member)}
                    >
                      <UserMinus className="mr-2 h-4 w-4" />
                      Remove
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              )}
            </div>
          );
        })}
      </div>

      {/* Remove Member Confirmation */}
      <AlertDialog open={!!removeMember} onOpenChange={() => setRemoveMember(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove Member</AlertDialogTitle>
            <AlertDialogDescription>
              Are you sure you want to remove {removeMember?.email} from this workspace? They will
              lose access to all workspace data.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={removeMutation.isPending}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() => removeMember && removeMutation.mutate(removeMember.user_id)}
              disabled={removeMutation.isPending}
            >
              {removeMutation.isPending ? 'Removing...' : 'Remove'}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
