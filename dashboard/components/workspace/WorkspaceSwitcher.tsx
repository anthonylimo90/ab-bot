'use client';

import { useEffect, useState } from 'react';
import { useRouter } from 'next/navigation';
import { Check, ChevronsUpDown, Building2, Plus } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { cn } from '@/lib/utils';

export function WorkspaceSwitcher() {
  const router = useRouter();
  const [open, setOpen] = useState(false);
  const {
    workspaces,
    currentWorkspace,
    isLoading,
    fetchWorkspaces,
    switchWorkspace,
    _hasHydrated,
  } = useWorkspaceStore();

  useEffect(() => {
    if (_hasHydrated) {
      fetchWorkspaces();
    }
  }, [_hasHydrated, fetchWorkspaces]);

  const handleSwitch = async (workspaceId: string) => {
    if (workspaceId === currentWorkspace?.id) {
      setOpen(false);
      return;
    }
    try {
      await switchWorkspace(workspaceId);
      setOpen(false);
      router.refresh();
    } catch {
      // Error handled in store
    }
  };

  if (!_hasHydrated || workspaces.length === 0) {
    return null;
  }

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Button
          variant="outline"
          role="combobox"
          aria-expanded={open}
          className="w-[200px] justify-between"
        >
          <div className="flex items-center gap-2 truncate">
            <Building2 className="h-4 w-4 shrink-0" />
            <span className="truncate">
              {currentWorkspace?.name || 'Select workspace'}
            </span>
          </div>
          <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-[200px]" align="start">
        <DropdownMenuLabel>Workspaces</DropdownMenuLabel>
        <DropdownMenuSeparator />
        {workspaces.map((workspace) => (
          <DropdownMenuItem
            key={workspace.id}
            onClick={() => handleSwitch(workspace.id)}
            disabled={isLoading}
            className="cursor-pointer"
          >
            <Check
              className={cn(
                'mr-2 h-4 w-4',
                currentWorkspace?.id === workspace.id ? 'opacity-100' : 'opacity-0'
              )}
            />
            <span className="truncate">{workspace.name}</span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
