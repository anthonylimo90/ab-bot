"use client";

import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";

export function useFlowFeaturesQuery(
  conditionId: string,
  windowMinutes?: number,
) {
  return useQuery({
    queryKey: queryKeys.signals.flow(conditionId, windowMinutes),
    queryFn: () =>
      api.getFlowFeatures({
        condition_id: conditionId,
        window_minutes: windowMinutes,
      }),
    enabled: Boolean(conditionId),
    staleTime: 60_000,
    refetchInterval: 5 * 60_000,
  });
}

export function useRecentSignalsQuery(params?: {
  kind?: string;
  limit?: number;
}) {
  return useQuery({
    queryKey: queryKeys.signals.recent(params),
    queryFn: () => api.getRecentSignals(params),
    staleTime: 15_000,
    refetchInterval: 30_000,
  });
}

export function useStrategyPerformanceQuery(periodDays?: number) {
  return useQuery({
    queryKey: queryKeys.signals.performance(periodDays),
    queryFn: () => api.getStrategyPerformance({ period_days: periodDays }),
    staleTime: 5 * 60_000,
    refetchInterval: 5 * 60_000,
  });
}

export function useMarketMetadataQuery(params?: {
  category?: string;
  active?: boolean;
  limit?: number;
}) {
  return useQuery({
    queryKey: queryKeys.signals.metadata(params),
    queryFn: () => api.getMarketMetadata(params),
    staleTime: 5 * 60_000,
  });
}

export function useMarketRegimeQuery() {
  return useQuery({
    queryKey: ["regime", "current"],
    queryFn: () => api.getMarketRegime(),
    staleTime: 5 * 60_000,
    refetchInterval: 5 * 60_000,
  });
}
