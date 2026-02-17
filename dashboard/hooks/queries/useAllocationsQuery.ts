"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import type {
  AddAllocationRequest,
  UpdateAllocationRequest,
} from "@/types/api";

export function useAllocationsQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: workspaceId
      ? queryKeys.allocations.list(workspaceId)
      : ["allocations", "disabled"],
    queryFn: () => api.listAllocations(),
    enabled: Boolean(workspaceId),
    staleTime: 60 * 1000,
  });
}

export function useActiveAllocationsQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: workspaceId
      ? queryKeys.allocations.active(workspaceId)
      : ["allocations", "active", "disabled"],
    queryFn: async () => {
      const allocations = await api.listAllocations();
      return allocations.filter((a) => a.tier === "active");
    },
    enabled: Boolean(workspaceId),
    staleTime: 60 * 1000,
  });
}

export function useBenchAllocationsQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: workspaceId
      ? queryKeys.allocations.bench(workspaceId)
      : ["allocations", "bench", "disabled"],
    queryFn: async () => {
      const allocations = await api.listAllocations();
      return allocations.filter((a) => a.tier === "bench");
    },
    enabled: Boolean(workspaceId),
    staleTime: 60 * 1000,
  });
}

function invalidateWorkspaceAllocationQueries(
  queryClient: ReturnType<typeof useQueryClient>,
  workspaceId: string | undefined,
) {
  if (!workspaceId) return;
  queryClient.invalidateQueries({
    queryKey: queryKeys.allocations.byWorkspace(workspaceId),
  });
}

export function useAddAllocationMutation(workspaceId: string | undefined) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      address,
      params,
    }: {
      address: string;
      params?: AddAllocationRequest;
    }) => api.addAllocation(address, params),
    onSuccess: () =>
      invalidateWorkspaceAllocationQueries(queryClient, workspaceId),
  });
}

export function useUpdateAllocationMutation(workspaceId: string | undefined) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      address,
      params,
    }: {
      address: string;
      params: UpdateAllocationRequest;
    }) => api.updateAllocation(address, params),
    onSuccess: () =>
      invalidateWorkspaceAllocationQueries(queryClient, workspaceId),
  });
}

export function usePromoteAllocationMutation(workspaceId: string | undefined) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (address: string) => api.promoteAllocation(address),
    onSuccess: () =>
      invalidateWorkspaceAllocationQueries(queryClient, workspaceId),
  });
}

export function useDemoteAllocationMutation(workspaceId: string | undefined) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (address: string) => api.demoteAllocation(address),
    onSuccess: () =>
      invalidateWorkspaceAllocationQueries(queryClient, workspaceId),
  });
}

export function useRemoveAllocationMutation(workspaceId: string | undefined) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (address: string) => api.removeAllocation(address),
    onSuccess: () =>
      invalidateWorkspaceAllocationQueries(queryClient, workspaceId),
  });
}

export function usePinAllocationMutation(workspaceId: string | undefined) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (address: string) => api.pinAllocation(address),
    onSuccess: () =>
      invalidateWorkspaceAllocationQueries(queryClient, workspaceId),
  });
}

export function useUnpinAllocationMutation(workspaceId: string | undefined) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (address: string) => api.unpinAllocation(address),
    onSuccess: () =>
      invalidateWorkspaceAllocationQueries(queryClient, workspaceId),
  });
}
