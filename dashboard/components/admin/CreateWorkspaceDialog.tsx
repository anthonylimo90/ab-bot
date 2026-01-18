'use client';

import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import api from '@/lib/api';
import type { SetupMode } from '@/types/api';

interface CreateWorkspaceDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function CreateWorkspaceDialog({ open, onOpenChange }: CreateWorkspaceDialogProps) {
  const queryClient = useQueryClient();
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [ownerEmail, setOwnerEmail] = useState('');
  const [setupMode, setSetupMode] = useState<SetupMode>('manual');
  const [error, setError] = useState<string | null>(null);

  const createMutation = useMutation({
    mutationFn: () =>
      api.adminCreateWorkspace({
        name,
        description: description || undefined,
        owner_email: ownerEmail,
        setup_mode: setupMode,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['admin', 'workspaces'] });
      onOpenChange(false);
      resetForm();
    },
    onError: (err: Error) => {
      setError(err.message);
    },
  });

  const resetForm = () => {
    setName('');
    setDescription('');
    setOwnerEmail('');
    setSetupMode('manual');
    setError(null);
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    createMutation.mutate();
  };

  const handleOpenChange = (open: boolean) => {
    if (!open) {
      resetForm();
    }
    onOpenChange(open);
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-[425px]">
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Create Workspace</DialogTitle>
            <DialogDescription>
              Create a new workspace and assign an owner. The owner will receive an email invite.
            </DialogDescription>
          </DialogHeader>

          <div className="grid gap-4 py-4">
            {error && (
              <div className="rounded-md bg-destructive/10 p-3 text-sm text-destructive">
                {error}
              </div>
            )}

            <div className="grid gap-2">
              <Label htmlFor="name">Workspace Name</Label>
              <Input
                id="name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="My Team Workspace"
                required
              />
            </div>

            <div className="grid gap-2">
              <Label htmlFor="description">Description (optional)</Label>
              <Input
                id="description"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="A brief description of the workspace"
              />
            </div>

            <div className="grid gap-2">
              <Label htmlFor="ownerEmail">Owner Email</Label>
              <Input
                id="ownerEmail"
                type="email"
                value={ownerEmail}
                onChange={(e) => setOwnerEmail(e.target.value)}
                placeholder="owner@example.com"
                required
              />
              <p className="text-xs text-muted-foreground">
                An invite will be sent to this email address
              </p>
            </div>

            <div className="grid gap-2">
              <Label htmlFor="setupMode">Initial Setup Mode</Label>
              <Select value={setupMode} onValueChange={(v) => setSetupMode(v as SetupMode)}>
                <SelectTrigger>
                  <SelectValue placeholder="Select setup mode" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="manual">
                    <div className="flex flex-col">
                      <span>Manual</span>
                      <span className="text-xs text-muted-foreground">
                        Owner selects wallets manually
                      </span>
                    </div>
                  </SelectItem>
                  <SelectItem value="automatic">
                    <div className="flex flex-col">
                      <span>Automatic</span>
                      <span className="text-xs text-muted-foreground">
                        System auto-selects best wallets
                      </span>
                    </div>
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => handleOpenChange(false)}
              disabled={createMutation.isPending}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={createMutation.isPending || !name || !ownerEmail}>
              {createMutation.isPending ? 'Creating...' : 'Create Workspace'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
