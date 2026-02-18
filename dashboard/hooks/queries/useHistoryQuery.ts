"use client";

import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";

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
