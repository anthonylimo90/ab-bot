"use client";

import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import type { Activity, DynamicConfigHistoryEntry } from "@/types/api";

export interface HistoryFilters {
  outcome?: "yes" | "no";
  copyTradesOnly?: boolean;
  market?: string;
  limit?: number;
  offset?: number;
}

export function useClosedPositionsQuery(filters?: HistoryFilters) {
  return useQuery({
    queryKey: queryKeys.positions.list({
      status: "closed",
      market: filters?.market,
      ...filters,
    }),
    queryFn: () =>
      api.getPositions({
        status: "closed",
        outcome: filters?.outcome,
        copy_trades_only: filters?.copyTradesOnly,
        market_id: filters?.market,
        limit: filters?.limit ?? 50,
        offset: filters?.offset ?? 0,
      }),
    staleTime: 60 * 1000,
  });
}

export interface ActivityHistoryFilters {
  limit?: number;
  offset?: number;
}

export function useActivityHistoryQuery(filters?: ActivityHistoryFilters) {
  return useQuery<Activity[]>({
    queryKey: [
      "activity-history",
      filters?.limit ?? 50,
      filters?.offset ?? 0,
    ],
    queryFn: () =>
      api.getActivity({
        limit: filters?.limit ?? 50,
        offset: filters?.offset ?? 0,
      }),
    staleTime: 30 * 1000,
  });
}

export interface DynamicHistoryFilters {
  workspaceId?: string;
  limit?: number;
  offset?: number;
}

export function useDynamicConfigHistoryQuery(filters?: DynamicHistoryFilters) {
  const workspaceId = filters?.workspaceId;
  return useQuery<DynamicConfigHistoryEntry[]>({
    queryKey: queryKeys.dynamicTuning.history(workspaceId ?? "", {
      limit: filters?.limit ?? 50,
      offset: filters?.offset ?? 0,
    }),
    queryFn: () =>
      api.getDynamicTuningHistory(workspaceId!, {
        limit: filters?.limit ?? 50,
        offset: filters?.offset ?? 0,
      }),
    enabled: Boolean(workspaceId),
    staleTime: 30 * 1000,
  });
}
