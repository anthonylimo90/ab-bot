"use client";

import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";

export function useRiskStatusQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.risk.status(workspaceId ?? ""),
    queryFn: () => api.getRiskStatus(workspaceId!),
    enabled: Boolean(workspaceId),
    refetchInterval: 15000,
    staleTime: 10000,
  });
}

export function useManualTripMutation(workspaceId: string | undefined) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => api.manualTripCircuitBreaker(workspaceId!),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.all() });
    },
  });
}

export function useResetCircuitBreakerMutation(
  workspaceId: string | undefined,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => api.resetCircuitBreaker(workspaceId!),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.all() });
    },
  });
}
