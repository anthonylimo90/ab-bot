"use client";

import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import { useToastStore } from "@/stores/toast-store";
import type { UpdateOpportunitySelectionRequest } from "@/types/api";

export function useRiskStatusQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.risk.status(),
    queryFn: () => api.getRiskStatus(workspaceId!),
    enabled: Boolean(workspaceId),
    refetchInterval: 15000,
    staleTime: 10000,
  });
}

export function useDynamicTunerQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.dynamicTuning.status(),
    queryFn: () => api.getDynamicTunerStatus(workspaceId!),
    enabled: Boolean(workspaceId),
    refetchInterval: 30000,
    staleTime: 15000,
  });
}

export function useServiceStatusQuery(workspaceId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.runtime.serviceStatus(),
    queryFn: () => api.getServiceStatus(workspaceId!),
    enabled: Boolean(workspaceId),
    refetchInterval: 30000,
    staleTime: 15000,
  });
}

export function useManualTripMutation(workspaceId: string | undefined) {
  const queryClient = useQueryClient();
  const toast = useToastStore();
  return useMutation({
    mutationFn: () => api.manualTripCircuitBreaker(workspaceId!),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.all() });
    },
    onError: (error: Error) => {
      // The breaker may have changed anyway (another operator, a retry):
      // show it as it is.
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.all() });
      toast.error("Circuit breaker trip failed", error.message);
    },
  });
}

export function useResetCircuitBreakerMutation(
  workspaceId: string | undefined,
) {
  const queryClient = useQueryClient();
  const toast = useToastStore();
  return useMutation({
    mutationFn: () => api.resetCircuitBreaker(workspaceId!),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.all() });
    },
    onError: (error: Error) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.all() });
      toast.error("Circuit breaker reset failed", error.message);
    },
  });
}

export function useUpdateOpportunitySelectionMutation(
  workspaceId: string | undefined,
) {
  const queryClient = useQueryClient();
  const toast = useToastStore();
  return useMutation({
    mutationFn: (params: UpdateOpportunitySelectionRequest) =>
      api.updateOpportunitySelection(workspaceId!, params),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.dynamicTuning.all(),
      });
    },
    onError: (error: Error) => {
      toast.error("Opportunity selection not saved", error.message);
    },
  });
}

export function useUpdateArbExecutorMutation(
  workspaceId: string | undefined,
) {
  const queryClient = useQueryClient();
  const toast = useToastStore();
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
    onError: (error: Error) => {
      toast.error("Arb executor settings not saved", error.message);
    },
  });
}
