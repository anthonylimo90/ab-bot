'use client';

import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { api } from '@/lib/api';
import { queryKeys } from '@/lib/queryClient';
import { useModeStore } from '@/stores/mode-store';

export function useOptimizerStatusQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.optimizer.status(workspaceId ?? ''),
    queryFn: () => api.getOptimizerStatus(workspaceId!),
    enabled: !!workspaceId,
    refetchInterval: 60000, // Refresh every minute
    staleTime: 30000,
  });
}

export function useRotationHistoryQuery(params?: {
  workspaceId?: string;
  limit?: number;
  unacknowledgedOnly?: boolean;
}) {
  const { mode } = useModeStore();

  return useQuery({
    queryKey: params?.workspaceId
      ? queryKeys.rotationHistory.list(mode, params.workspaceId, { unacknowledgedOnly: params?.unacknowledgedOnly })
      : ['rotation-history', mode, 'disabled'],
    queryFn: () =>
      api.listRotationHistory({
        limit: params?.limit,
        unacknowledged_only: params?.unacknowledgedOnly,
      }),
    enabled: Boolean(params?.workspaceId) && mode === 'live',
    refetchInterval: 30000, // Refresh every 30 seconds
  });
}

export function useTriggerOptimizationMutation() {
  const queryClient = useQueryClient();
  const { mode } = useModeStore();

  return useMutation({
    mutationFn: () => api.triggerOptimization(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.optimizer.all });
      queryClient.invalidateQueries({ queryKey: queryKeys.rotationHistory.all(mode) });
      queryClient.invalidateQueries({ queryKey: queryKeys.allocations.all(mode) });
    },
  });
}

export function useAcknowledgeRotationMutation() {
  const queryClient = useQueryClient();
  const { mode } = useModeStore();

  return useMutation({
    mutationFn: (entryId: string) => api.acknowledgeRotation(entryId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.rotationHistory.all(mode) });
    },
  });
}

// Hook to get unacknowledged rotation count
export function useUnacknowledgedRotationCount(workspaceId: string | undefined) {
  const { data: history } = useRotationHistoryQuery({ unacknowledgedOnly: true, workspaceId });
  return history?.length ?? 0;
}
