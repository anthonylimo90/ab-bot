"use client";

import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";

interface DiscoverFilters {
  sortBy?: "roi" | "sharpe" | "winRate" | "trades" | "composite";
  period?: "7d" | "30d" | "90d";
  minTrades?: number;
  minWinRate?: number;
  limit?: number;
  workspaceId?: string;
}

export function useDiscoverWalletsQuery(filters?: DiscoverFilters) {
  return useQuery({
    queryKey: queryKeys.discover.wallets(filters, filters?.workspaceId),
    queryFn: () =>
      api.discoverWallets({
        sort_by: filters?.sortBy,
        period: filters?.period,
        min_trades: filters?.minTrades,
        min_win_rate: filters?.minWinRate,
        limit: filters?.limit,
      }),
    staleTime: 60 * 1000,
  });
}

export function useLiveTradesQuery(params?: {
  wallet?: string;
  limit?: number;
  minValue?: number;
  workspaceId?: string;
}) {
  return useQuery({
    queryKey: queryKeys.discover.trades(
      {
        wallet: params?.wallet,
        limit: params?.limit,
        minValue: params?.minValue,
      },
      params?.workspaceId,
    ),
    queryFn: () =>
      api.getLiveTrades({
        wallet: params?.wallet,
        limit: params?.limit,
        min_value: params?.minValue,
      }),
    staleTime: 10 * 1000,
    refetchInterval: 15 * 1000,
  });
}

export function useDiscoveredWalletQuery(address: string, enabled = true) {
  return useQuery({
    queryKey: ["discover", "wallet", address],
    queryFn: () => api.getDiscoveredWallet(address),
    enabled: Boolean(address) && enabled,
    staleTime: 60 * 1000,
    retry: false,
  });
}

export function useMarketRegimeQuery() {
  return useQuery({
    queryKey: ["regime", "current"],
    queryFn: () => api.getMarketRegime(),
    staleTime: 5 * 60 * 1000, // 5 minutes
    refetchInterval: 5 * 60 * 1000, // Refresh every 5 minutes
  });
}

export function useCalibrationReportQuery() {
  return useQuery({
    queryKey: ["discover", "calibration"],
    queryFn: () => api.getCalibrationReport(),
    staleTime: 10 * 60 * 1000, // 10 minutes
    refetchInterval: 10 * 60 * 1000,
  });
}

export function useCopyPerformanceQuery(address: string, enabled = true) {
  return useQuery({
    queryKey: ["discover", "copy-performance", address],
    queryFn: () => api.getCopyPerformance(address),
    enabled: Boolean(address) && enabled,
    staleTime: 5 * 60 * 1000,
  });
}

export function useLeaderboardQuery(workspaceId?: string) {
  return useDiscoverWalletsQuery({
    sortBy: "roi",
    period: "30d",
    minTrades: 10,
    limit: 10,
    ...(workspaceId ? { workspaceId } : {}),
  });
}
