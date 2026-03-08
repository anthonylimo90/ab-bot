"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";

export function useAccountSummaryQuery(workspaceId?: string | null) {
  return useQuery({
    queryKey: workspaceId
      ? queryKeys.account.summary(workspaceId)
      : ["account", "summary", "disabled"],
    queryFn: () => api.getAccountSummary(workspaceId!),
    enabled: Boolean(workspaceId),
    staleTime: 15_000,
    refetchInterval: 30_000,
  });
}

export function useAccountHistoryQuery(
  workspaceId?: string | null,
  params?: { hours?: number; limit?: number },
) {
  return useQuery({
    queryKey: workspaceId
      ? queryKeys.account.history(workspaceId, params)
      : ["account", "history", "disabled", params],
    queryFn: () => api.getAccountHistory(workspaceId!, params),
    enabled: Boolean(workspaceId),
    staleTime: 15_000,
    refetchInterval: 60_000,
  });
}

export function useCashFlowsQuery(
  workspaceId?: string | null,
  params?: { limit?: number },
) {
  return useQuery({
    queryKey: workspaceId
      ? queryKeys.account.cashFlows(workspaceId, params)
      : ["account", "cash-flows", "disabled", params],
    queryFn: () => api.getCashFlows(workspaceId!, params),
    enabled: Boolean(workspaceId),
    staleTime: 15_000,
    refetchInterval: 60_000,
  });
}

export function useCreateCashFlowMutation(workspaceId?: string | null) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (params: {
      event_type: string;
      amount: number;
      note?: string;
      occurred_at?: string;
    }) => api.createCashFlow(workspaceId!, params),
    onSuccess: () => {
      if (!workspaceId) return;
      queryClient.invalidateQueries({ queryKey: queryKeys.account.summary(workspaceId) });
      queryClient.invalidateQueries({ queryKey: queryKeys.account.history(workspaceId) });
      queryClient.invalidateQueries({ queryKey: queryKeys.account.cashFlows(workspaceId) });
    },
  });
}
