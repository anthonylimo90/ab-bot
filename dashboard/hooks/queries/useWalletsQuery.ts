'use client';

import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { api } from '@/lib/api';
import { queryKeys } from '@/lib/queryClient';
import type { Wallet, WalletMetrics } from '@/types/api';
import type { TradingMode } from '@/stores/mode-store';

interface WalletFilters {
  copyEnabled?: boolean;
  minScore?: number;
}

export function useWalletsQuery(mode: TradingMode, filters?: WalletFilters) {
  return useQuery({
    queryKey: queryKeys.wallets.all(mode),
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

export function useWalletQuery(mode: TradingMode, address: string) {
  return useQuery({
    queryKey: queryKeys.wallets.detail(mode, address),
    queryFn: () => api.getWallet(address),
    enabled: !!address,
  });
}

export function useWalletMetricsQuery(mode: TradingMode, address: string) {
  return useQuery({
    queryKey: queryKeys.wallets.metrics(mode, address),
    queryFn: () => api.getWalletMetrics(address),
    enabled: !!address,
    staleTime: 5 * 60 * 1000,
  });
}

export function useTrackWalletMutation(mode: TradingMode) {
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
      queryClient.invalidateQueries({ queryKey: queryKeys.wallets.all(mode) });
    },
  });
}

export function useUntrackWalletMutation(mode: TradingMode) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (address: string) => {
      await api.deleteWallet(address);
      return { success: true };
    },
    onSuccess: (_, address) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.wallets.all(mode) });
      queryClient.removeQueries({
        queryKey: queryKeys.wallets.detail(mode, address),
      });
    },
  });
}

export function useUpdateWalletMutation(mode: TradingMode) {
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
      queryClient.invalidateQueries({ queryKey: queryKeys.wallets.all(mode) });
      queryClient.invalidateQueries({
        queryKey: queryKeys.wallets.detail(mode, address),
      });
    },
  });
}

export function useWalletBalanceQuery(address: string | null) {
  return useQuery({
    queryKey: ['wallet-balance', address],
    queryFn: () => api.getWalletBalance(address!),
    enabled: !!address,
    staleTime: 30_000,
    refetchInterval: 60_000,
    retry: 1,
  });
}

// Derived hooks for roster/bench separation
export function useRosterWallets(mode: TradingMode) {
  const query = useWalletsQuery(mode);

  return {
    ...query,
    rosterWallets: (query.data ?? []).filter((w) => w.copy_enabled),
  };
}

export function useBenchWallets(mode: TradingMode) {
  const query = useWalletsQuery(mode);

  return {
    ...query,
    benchWallets: (query.data ?? []).filter((w) => !w.copy_enabled),
  };
}

// Recommendations query - fetches top wallets for discovery
export function useRecommendationsQuery(limit = 5) {
  return useQuery({
    queryKey: ['recommendations', limit],
    queryFn: async () => {
      const wallets = await api.discoverWallets({
        sort_by: 'roi',
        period: '30d',
        min_trades: 10,
        min_win_rate: 50,
        limit,
      });
      return wallets;
    },
    staleTime: 60 * 1000,
  });
}
