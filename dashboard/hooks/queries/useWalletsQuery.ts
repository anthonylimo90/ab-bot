"use client";

import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import type { Wallet, WalletMetrics, WalletTrade } from "@/types/api";

interface WalletFilters {
  copyEnabled?: boolean;
  minScore?: number;
}

export function useWalletsQuery(filters?: WalletFilters) {
  return useQuery({
    queryKey: queryKeys.wallets.all(),
    queryFn: async () => {
      const data = await api.getWallets({
        copy_enabled: filters?.copyEnabled,
        min_score: filters?.minScore,
      });
      return data;
    },
    staleTime: 60 * 1000,
  });
}

export function useWalletQuery(address: string, enabled = true) {
  return useQuery({
    queryKey: queryKeys.wallets.detail(address),
    queryFn: () => api.getWallet(address),
    enabled: Boolean(address) && enabled,
  });
}

export function useWalletMetricsQuery(address: string, enabled = true) {
  return useQuery({
    queryKey: queryKeys.wallets.metrics(address),
    queryFn: () => api.getWalletMetrics(address),
    enabled: Boolean(address) && enabled,
    staleTime: 5 * 60 * 1000,
  });
}

export function useWalletTradesQuery(
  address: string,
  params?: {
    limit?: number;
    offset?: number;
  },
) {
  return useQuery({
    queryKey: [...queryKeys.wallets.all(), "trades", address, params] as const,
    queryFn: () => api.getWalletTrades(address, params),
    enabled: Boolean(address),
    staleTime: 30 * 1000,
  });
}

export function useTrackWalletMutation() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      address,
      label,
    }: {
      address: string;
      label?: string;
    }) => {
      return api.addWallet({ address, label });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.wallets.all() });
    },
  });
}

export function useUntrackWalletMutation() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (address: string) => {
      await api.deleteWallet(address);
      return { success: true };
    },
    onSuccess: (_, address) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.wallets.all() });
      queryClient.removeQueries({
        queryKey: queryKeys.wallets.detail(address),
      });
    },
  });
}

export function useUpdateWalletMutation() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      address,
      params,
    }: {
      address: string;
      params: {
        label?: string;
        copy_enabled?: boolean;
        allocation_pct?: number;
        max_position_size?: number;
      };
    }) => {
      return api.updateWallet(address, params);
    },
    onSuccess: (_, { address }) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.wallets.all() });
      queryClient.invalidateQueries({
        queryKey: queryKeys.wallets.detail(address),
      });
    },
  });
}

export function useWalletBalanceQuery(address: string | null) {
  return useQuery({
    queryKey: ["wallet-balance", address],
    queryFn: () => api.getWalletBalance(address!),
    enabled: Boolean(address),
    staleTime: 30_000,
    refetchInterval: 60_000,
    placeholderData: (previousData) => previousData,
    refetchOnWindowFocus: false,
    retry: 1,
  });
}

// Derived hooks for roster/bench separation
export function useRosterWallets() {
  const query = useWalletsQuery();

  return {
    ...query,
    rosterWallets: (query.data ?? []).filter((w) => w.copy_enabled),
  };
}

export function useBenchWallets() {
  const query = useWalletsQuery();

  return {
    ...query,
    benchWallets: (query.data ?? []).filter((w) => !w.copy_enabled),
  };
}

// Recommendations query - fetches top wallets for discovery
export function useRecommendationsQuery(limit = 5) {
  return useQuery({
    queryKey: ["recommendations", limit],
    queryFn: async () => {
      const wallets = await api.discoverWallets({
        sort_by: "roi",
        period: "30d",
        min_trades: 10,
        min_win_rate: 50,
        limit,
      });
      return wallets;
    },
    staleTime: 60 * 1000,
  });
}
