"use client";

import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import type { UpdateOpportunitySelectionRequest } from "@/types/api";

export function useRiskStatusQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.risk.status(workspaceId ?? ""),
    queryFn: () => api.getRiskStatus(workspaceId!),
    enabled: Boolean(workspaceId),
    refetchInterval: 15000,
    staleTime: 10000,
  });
}

export function useDynamicTunerQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.dynamicTuning.status(workspaceId ?? ""),
    queryFn: () => api.getDynamicTunerStatus(workspaceId!),
    enabled: Boolean(workspaceId),
    refetchInterval: 30000,
    staleTime: 15000,
  });
}

export function useServiceStatusQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: ["service-status", workspaceId],
    queryFn: () => api.getServiceStatus(workspaceId!),
    enabled: Boolean(workspaceId),
    refetchInterval: 30000,
    staleTime: 15000,
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

export function useUpdateOpportunitySelectionMutation(
  workspaceId: string | undefined,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (params: UpdateOpportunitySelectionRequest) =>
      api.updateOpportunitySelection(workspaceId!, params),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.dynamicTuning.all(),
      });
    },
  });
}

export function useUpdateArbExecutorMutation(
  workspaceId: string | undefined,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (params: {
      position_size?: number;
      min_net_profit?: number;
      min_book_depth?: number;
      max_signal_age_secs?: number;
    }) => api.updateArbExecutorConfig(workspaceId!, params),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.dynamicTuning.all(),
      });
    },
  });
}
