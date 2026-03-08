"use client";

import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";

type TradeFlowParams = {
  from?: string;
  to?: string;
  strategy?: string;
  limit?: number;
};

export function useTradeFlowSummaryQuery(params?: TradeFlowParams) {
  return useQuery({
    queryKey: queryKeys.tradeFlow.summary(params),
    queryFn: () => api.getTradeFlowSummary(params),
    staleTime: 15_000,
    refetchInterval: 30_000,
  });
}

export function useTradeFlowJourneysQuery(params?: TradeFlowParams) {
  return useQuery({
    queryKey: queryKeys.tradeFlow.journeys(params),
    queryFn: () => api.getTradeFlowJourneys(params),
    staleTime: 15_000,
    refetchInterval: 30_000,
  });
}

export function useMarketTradeFlowQuery(
  marketId: string,
  params?: TradeFlowParams,
) {
  return useQuery({
    queryKey: queryKeys.tradeFlow.market(marketId, params),
    queryFn: () => api.getMarketTradeFlow(marketId, params),
    enabled: Boolean(marketId),
    staleTime: 15_000,
    refetchInterval: 30_000,
  });
}
