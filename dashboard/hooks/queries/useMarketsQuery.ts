"use client";

import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";

interface MarketFilters {
  category?: string;
  active?: boolean;
  min_volume?: number;
  limit?: number;
  offset?: number;
}

export function useMarketsQuery(params?: MarketFilters) {
  return useQuery({
    queryKey: queryKeys.markets.list(params),
    queryFn: () => api.getMarkets(params),
    staleTime: 60_000,
  });
}

export function useMarketQuery(marketId: string | null) {
  return useQuery({
    queryKey: queryKeys.markets.detail(marketId ?? ""),
    queryFn: () => api.getMarket(marketId!),
    enabled: !!marketId,
    staleTime: 60_000,
  });
}

export function useOrderbookQuery(marketId: string | null, enabled = true) {
  return useQuery({
    queryKey: queryKeys.markets.orderbook(marketId ?? ""),
    queryFn: () => api.getOrderbook(marketId!),
    enabled: !!marketId && enabled,
    staleTime: 10_000,
    refetchInterval: 15_000,
  });
}
