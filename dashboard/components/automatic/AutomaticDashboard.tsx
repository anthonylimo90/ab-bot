'use client';

import { useState } from 'react';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { useToastStore } from '@/stores/toast-store';
import {
  useOptimizerStatusQuery,
  useRotationHistoryQuery,
  useActiveAllocationsQuery,
  useTriggerOptimizationMutation,
  useAcknowledgeRotationMutation,
} from '@/hooks/queries';
import { api } from '@/lib/api';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { PortfolioSummaryCard } from './PortfolioSummaryCard';
import { OptimizerStatusCard } from './OptimizerStatusCard';
import { ActiveWalletsCard } from './ActiveWalletsCard';
import { RotationHistoryCard } from './RotationHistoryCard';
import { QuickSettingsDialog } from './QuickSettingsDialog';
import { Badge } from '@/components/ui/badge';
import { Bot } from 'lucide-react';
import type { UpdateWorkspaceRequest } from '@/types/api';

export function AutomaticDashboard() {
  const toast = useToastStore();
  const queryClient = useQueryClient();
  const { currentWorkspace } = useWorkspaceStore();
  const [settingsOpen, setSettingsOpen] = useState(false);

  // Queries
  const { data: optimizerStatus, isLoading: statusLoading } = useOptimizerStatusQuery(
    currentWorkspace?.id
  );
  const { data: rotationHistory, isLoading: historyLoading } = useRotationHistoryQuery({
    limit: 10,
  });
  const { data: activeWallets, isLoading: walletsLoading } = useActiveAllocationsQuery();

  // Mutations
  const triggerOptimization = useTriggerOptimizationMutation();
  const acknowledgeRotation = useAcknowledgeRotationMutation();

  const updateWorkspace = useMutation({
    mutationFn: async (updates: UpdateWorkspaceRequest) => {
      if (!currentWorkspace?.id) throw new Error('No workspace selected');
      return api.updateWorkspace(currentWorkspace.id, updates);
    },
    onSuccess: () => {
      toast.success('Settings saved', 'Optimizer settings have been updated');
      queryClient.invalidateQueries({ queryKey: ['optimizer'] });
      setSettingsOpen(false);
    },
    onError: (error: Error) => {
      toast.error('Failed to save', error.message);
    },
  });

  const handleTriggerOptimization = () => {
    triggerOptimization.mutate(undefined, {
      onSuccess: () => {
        toast.success('Optimization started', 'The optimizer is now running');
      },
      onError: (error: Error) => {
        toast.error('Optimization failed', error.message);
      },
    });
  };

  const handleAcknowledge = (entryId: string) => {
    acknowledgeRotation.mutate(entryId, {
      onSuccess: () => {
        toast.success('Acknowledged', 'Rotation entry has been acknowledged');
      },
      onError: (error: Error) => {
        toast.error('Failed to acknowledge', error.message);
      },
    });
  };

  const handleSaveSettings = async (updates: UpdateWorkspaceRequest) => {
    await updateWorkspace.mutateAsync(updates);
  };

  // Check if user can trigger (owner or admin)
  const canTrigger = ['owner', 'admin'].includes(currentWorkspace?.my_role ?? '');

  return (
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <div className="flex items-center gap-3">
            <h1 className="text-3xl font-bold tracking-tight">Dashboard</h1>
            <Badge variant="secondary" className="text-sm">
              <Bot className="h-3 w-3 mr-1" />
              Automatic Mode
            </Badge>
          </div>
          <p className="text-muted-foreground">
            Auto-optimized portfolio performance
          </p>
        </div>
      </div>

      {/* Portfolio Summary */}
      <PortfolioSummaryCard
        metrics={optimizerStatus?.portfolio_metrics}
        isLoading={statusLoading}
      />

      {/* Main Grid */}
      <div className="grid gap-6 lg:grid-cols-2">
        {/* Left Column */}
        <div className="space-y-6">
          <OptimizerStatusCard
            status={optimizerStatus}
            isLoading={statusLoading}
            onTriggerOptimization={handleTriggerOptimization}
            isTriggering={triggerOptimization.isPending}
            onOpenSettings={() => setSettingsOpen(true)}
            canTrigger={canTrigger}
          />
          <ActiveWalletsCard wallets={activeWallets} isLoading={walletsLoading} />
        </div>

        {/* Right Column */}
        <RotationHistoryCard
          history={rotationHistory}
          isLoading={historyLoading}
          onAcknowledge={handleAcknowledge}
          isAcknowledging={acknowledgeRotation.isPending}
        />
      </div>

      {/* Settings Dialog */}
      <QuickSettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        workspace={currentWorkspace}
        onSave={handleSaveSettings}
        isSaving={updateWorkspace.isPending}
      />
    </div>
  );
}
