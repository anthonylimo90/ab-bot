'use client';

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '@/lib/api';
import { queryKeys } from '@/lib/queryClient';
import type { AddAllocationRequest, UpdateAllocationRequest } from '@/types/api';
import type { TradingMode } from '@/stores/mode-store';

export function useAllocationsQuery(workspaceId: string | undefined, mode: TradingMode) {
  return useQuery({
    queryKey: workspaceId ? queryKeys.allocations.list(mode, workspaceId) : ['allocations', mode, 'disabled'],
    queryFn: () => api.listAllocations(),
    enabled: Boolean(workspaceId) && mode === 'live',
    staleTime: 60 * 1000,
  });
}

export function useActiveAllocationsQuery(workspaceId: string | undefined, mode: TradingMode) {
  return useQuery({
    queryKey: workspaceId ? queryKeys.allocations.active(mode, workspaceId) : ['allocations', mode, 'active', 'disabled'],
    queryFn: async () => {
      const allocations = await api.listAllocations();
      return allocations.filter((a) => a.tier === 'active');
    },
    enabled: Boolean(workspaceId) && mode === 'live',
    staleTime: 60 * 1000,
  });
}

export function useBenchAllocationsQuery(workspaceId: string | undefined, mode: TradingMode) {
  return useQuery({
    queryKey: workspaceId ? queryKeys.allocations.bench(mode, workspaceId) : ['allocations', mode, 'bench', 'disabled'],
    queryFn: async () => {
      const allocations = await api.listAllocations();
      return allocations.filter((a) => a.tier === 'bench');
    },
    enabled: Boolean(workspaceId) && mode === 'live',
    staleTime: 60 * 1000,
  });
}

function invalidateWorkspaceAllocationQueries(queryClient: ReturnType<typeof useQueryClient>, mode: TradingMode, workspaceId: string | undefined) {
  if (!workspaceId) return;
  queryClient.invalidateQueries({ queryKey: queryKeys.allocations.byWorkspace(mode, workspaceId) });
}

export function useAddAllocationMutation(workspaceId: string | undefined, mode: TradingMode) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ address, params }: { address: string; params?: AddAllocationRequest }) =>
      api.addAllocation(address, params),
    onSuccess: () => invalidateWorkspaceAllocationQueries(queryClient, mode, workspaceId),
  });
}

export function useUpdateAllocationMutation(workspaceId: string | undefined, mode: TradingMode) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ address, params }: { address: string; params: UpdateAllocationRequest }) =>
      api.updateAllocation(address, params),
    onSuccess: () => invalidateWorkspaceAllocationQueries(queryClient, mode, workspaceId),
  });
}

export function usePromoteAllocationMutation(workspaceId: string | undefined, mode: TradingMode) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (address: string) => api.promoteAllocation(address),
    onSuccess: () => invalidateWorkspaceAllocationQueries(queryClient, mode, workspaceId),
  });
}

export function useDemoteAllocationMutation(workspaceId: string | undefined, mode: TradingMode) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (address: string) => api.demoteAllocation(address),
    onSuccess: () => invalidateWorkspaceAllocationQueries(queryClient, mode, workspaceId),
  });
}

export function useRemoveAllocationMutation(workspaceId: string | undefined, mode: TradingMode) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (address: string) => api.removeAllocation(address),
    onSuccess: () => invalidateWorkspaceAllocationQueries(queryClient, mode, workspaceId),
  });
}

export function usePinAllocationMutation(workspaceId: string | undefined, mode: TradingMode) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (address: string) => api.pinAllocation(address),
    onSuccess: () => invalidateWorkspaceAllocationQueries(queryClient, mode, workspaceId),
  });
}

export function useUnpinAllocationMutation(workspaceId: string | undefined, mode: TradingMode) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (address: string) => api.unpinAllocation(address),
    onSuccess: () => invalidateWorkspaceAllocationQueries(queryClient, mode, workspaceId),
  });
}
