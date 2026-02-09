'use client';

import { useQuery } from '@tanstack/react-query';
import { api } from '@/lib/api';
import { queryKeys } from '@/lib/queryClient';
import type { DiscoveredWallet, LiveTrade, DemoPnlSimulation } from '@/types/api';
import type { TradingMode } from '@/stores/mode-store';

interface DiscoverFilters {
  sortBy?: 'roi' | 'sharpe' | 'winRate' | 'trades';
  period?: '7d' | '30d' | '90d';
  minTrades?: number;
  minWinRate?: number;
  limit?: number;
  workspaceId?: string;
}

export function useDiscoverWalletsQuery(mode: TradingMode, filters?: DiscoverFilters) {
  return useQuery({
    queryKey: queryKeys.discover.wallets(mode, filters, filters?.workspaceId),
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

export function useLiveTradesQuery(mode: TradingMode, params?: {
  wallet?: string;
  limit?: number;
  minValue?: number;
  workspaceId?: string;
}) {
  return useQuery({
    queryKey: queryKeys.discover.trades(
      mode,
      { wallet: params?.wallet, limit: params?.limit, minValue: params?.minValue },
      params?.workspaceId
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

export function useDemoPnlSimulationQuery(mode: TradingMode, params?: {
  amount?: number;
  period?: '7d' | '30d' | '90d';
  wallets?: string[];
  workspaceId?: string;
}) {
  return useQuery({
    queryKey: queryKeys.discover.simulate(
      mode,
      { amount: params?.amount, period: params?.period, wallets: params?.wallets },
      params?.workspaceId
    ),
    queryFn: () =>
      api.simulateDemoPnl({
        amount: params?.amount,
        period: params?.period,
        wallets: params?.wallets?.join(','),
      }),
    staleTime: 5 * 60 * 1000,
  });
}

export function useDiscoveredWalletQuery(mode: TradingMode, address: string) {
  return useQuery({
    queryKey: ['discover', 'wallet', mode, address],
    queryFn: () => api.getDiscoveredWallet(address),
    enabled: !!address,
    staleTime: 60 * 1000,
    retry: false,
  });
}

export function useLeaderboardQuery(mode: TradingMode, workspaceId?: string) {
  return useDiscoverWalletsQuery(mode, {
    sortBy: 'roi',
    period: '30d',
    minTrades: 10,
    limit: 10,
    ...(workspaceId ? { workspaceId } : {}),
  });
}
