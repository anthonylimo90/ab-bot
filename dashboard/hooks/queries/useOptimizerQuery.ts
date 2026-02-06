'use client';

import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { api } from '@/lib/api';
import { queryKeys } from '@/lib/queryClient';

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
  return useQuery({
    queryKey: params?.workspaceId
      ? queryKeys.rotationHistory.list(params.workspaceId, { unacknowledgedOnly: params?.unacknowledgedOnly })
      : ['rotation-history', 'disabled'],
    queryFn: () =>
      api.listRotationHistory({
        limit: params?.limit,
        unacknowledged_only: params?.unacknowledgedOnly,
      }),
    enabled: Boolean(params?.workspaceId),
    refetchInterval: 30000, // Refresh every 30 seconds
  });
}

export function useActiveAllocationsQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: workspaceId ? queryKeys.allocations.active(workspaceId) : ['allocations', 'active', 'disabled'],
    queryFn: async () => {
      const allocations = await api.listAllocations();
      return allocations.filter((a) => a.tier === 'active');
    },
    enabled: Boolean(workspaceId),
    staleTime: 60000,
  });
}

export function useBenchAllocationsQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: workspaceId ? queryKeys.allocations.bench(workspaceId) : ['allocations', 'bench', 'disabled'],
    queryFn: async () => {
      const allocations = await api.listAllocations();
      return allocations.filter((a) => a.tier === 'bench');
    },
    enabled: Boolean(workspaceId),
    staleTime: 60000,
  });
}

export function useTriggerOptimizationMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => api.triggerOptimization(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.optimizer.all });
      queryClient.invalidateQueries({ queryKey: queryKeys.rotationHistory.all });
      queryClient.invalidateQueries({ queryKey: queryKeys.allocations.all });
    },
  });
}

export function useAcknowledgeRotationMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (entryId: string) => api.acknowledgeRotation(entryId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.rotationHistory.all });
    },
  });
}

// Hook to get unacknowledged rotation count
export function useUnacknowledgedRotationCount(workspaceId: string | undefined) {
  const { data: history } = useRotationHistoryQuery({ unacknowledgedOnly: true, workspaceId });
  return history?.length ?? 0;
}
