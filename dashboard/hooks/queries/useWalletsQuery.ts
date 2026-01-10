'use client';

import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useModeStore } from '@/stores/mode-store';
import { api } from '@/lib/api';
import { queryKeys } from '@/lib/queryClient';
import type { Wallet, WalletMetrics } from '@/types/api';

// Generate mock equity curve data
function generateEquityCurve(): { time: string; value: number }[] {
  const data: { time: string; value: number }[] = [];
  let value = 1000;
  const now = Date.now();

  for (let i = 30; i >= 0; i--) {
    const date = new Date(now - i * 24 * 60 * 60 * 1000);
    value = value * (1 + (Math.random() - 0.45) * 0.05);
    data.push({
      time: date.toISOString().split('T')[0],
      value: Math.round(value * 100) / 100,
    });
  }

  return data;
}

// Mock wallets for demo mode
const mockWallets: Wallet[] = [
  {
    address: '0x1234567890abcdef1234567890abcdef12345678',
    label: 'Top Trader Alpha',
    copy_enabled: true,
    allocation_pct: 30,
    max_position_size: 1000,
    success_score: 85,
    total_pnl: 15420.5,
    win_rate: 72,
    total_trades: 156,
    added_at: new Date(Date.now() - 30 * 24 * 60 * 60 * 1000).toISOString(),
    last_activity: new Date(Date.now() - 3600000).toISOString(),
    equity_curve: generateEquityCurve(),
    prediction: {
      success_probability: 0.85,
      confidence: 0.92,
      category: 'HIGH_POTENTIAL',
    },
  },
  {
    address: '0xabcdef1234567890abcdef1234567890abcdef12',
    label: 'Momentum Master',
    copy_enabled: true,
    allocation_pct: 25,
    max_position_size: 800,
    success_score: 78,
    total_pnl: 8930.25,
    win_rate: 68,
    total_trades: 89,
    added_at: new Date(Date.now() - 45 * 24 * 60 * 60 * 1000).toISOString(),
    last_activity: new Date(Date.now() - 7200000).toISOString(),
    equity_curve: generateEquityCurve(),
    prediction: {
      success_probability: 0.78,
      confidence: 0.85,
      category: 'HIGH_POTENTIAL',
    },
  },
  {
    address: '0x9876543210fedcba9876543210fedcba98765432',
    label: 'Value Hunter',
    copy_enabled: false,
    allocation_pct: 0,
    max_position_size: 500,
    success_score: 71,
    total_pnl: 5240.0,
    win_rate: 65,
    total_trades: 203,
    added_at: new Date(Date.now() - 60 * 24 * 60 * 60 * 1000).toISOString(),
    last_activity: new Date(Date.now() - 14400000).toISOString(),
    equity_curve: generateEquityCurve(),
    prediction: {
      success_probability: 0.71,
      confidence: 0.78,
      category: 'MODERATE',
    },
  },
  {
    address: '0xfedcba9876543210fedcba9876543210fedcba98',
    label: 'Steady Eddie',
    copy_enabled: false,
    allocation_pct: 0,
    max_position_size: 300,
    success_score: 68,
    total_pnl: 3180.75,
    win_rate: 62,
    total_trades: 178,
    added_at: new Date(Date.now() - 90 * 24 * 60 * 60 * 1000).toISOString(),
    last_activity: new Date(Date.now() - 28800000).toISOString(),
    equity_curve: generateEquityCurve(),
    prediction: {
      success_probability: 0.68,
      confidence: 0.72,
      category: 'MODERATE',
    },
  },
];

interface WalletFilters {
  copyEnabled?: boolean;
  minScore?: number;
}

export function useWalletsQuery(filters?: WalletFilters) {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';

  return useQuery({
    queryKey: queryKeys.wallets.all,
    queryFn: async () => {
      if (!isLiveMode) {
        await new Promise((resolve) => setTimeout(resolve, 400));
        let wallets = [...mockWallets];

        // Apply filters for demo mode
        if (filters?.copyEnabled !== undefined) {
          wallets = wallets.filter(
            (w) => w.copy_enabled === filters.copyEnabled
          );
        }
        if (filters?.minScore !== undefined) {
          wallets = wallets.filter(
            (w) => (w.success_score ?? 0) >= filters.minScore!
          );
        }

        return wallets;
      }

      const data = await api.getWallets({
        copy_enabled: filters?.copyEnabled,
        min_score: filters?.minScore,
      });

      // Enrich with equity curves (would come from API in production)
      return data.map((wallet) => ({
        ...wallet,
        equity_curve: generateEquityCurve(),
      }));
    },
    staleTime: isLiveMode ? 60 * 1000 : 5 * 60 * 1000, // Wallet data changes less frequently
  });
}

export function useWalletQuery(address: string) {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';

  return useQuery({
    queryKey: queryKeys.wallets.detail(address),
    queryFn: async () => {
      if (!isLiveMode) {
        await new Promise((resolve) => setTimeout(resolve, 200));
        const wallet = mockWallets.find(
          (w) => w.address.toLowerCase() === address.toLowerCase()
        );
        if (!wallet) throw new Error('Wallet not found');
        return wallet;
      }

      const wallet = await api.getWallet(address);
      return { ...wallet, equity_curve: generateEquityCurve() };
    },
    enabled: !!address,
  });
}

export function useWalletMetricsQuery(address: string) {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';

  return useQuery({
    queryKey: queryKeys.wallets.metrics(address),
    queryFn: async (): Promise<WalletMetrics> => {
      if (!isLiveMode) {
        await new Promise((resolve) => setTimeout(resolve, 300));
        return {
          address,
          roi: 47.3,
          sharpe_ratio: 2.4,
          max_drawdown: -8.2,
          avg_trade_size: 150,
          avg_hold_time_hours: 24,
          profit_factor: 2.1,
          recent_pnl_30d: 1250.5,
          category_win_rates: { politics: 0.75, crypto: 0.68, sports: 0.62 },
          calculated_at: new Date().toISOString(),
        };
      }

      return api.getWalletMetrics(address);
    },
    enabled: !!address,
    staleTime: 5 * 60 * 1000, // Metrics don't change very frequently
  });
}

export function useTrackWalletMutation() {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      address,
      label,
    }: {
      address: string;
      label?: string;
    }) => {
      if (!isLiveMode) {
        await new Promise((resolve) => setTimeout(resolve, 500));
        const newWallet: Wallet = {
          address,
          label,
          copy_enabled: false,
          allocation_pct: 0,
          max_position_size: 500,
          success_score: 50,
          total_pnl: 0,
          win_rate: 0,
          total_trades: 0,
          added_at: new Date().toISOString(),
          equity_curve: generateEquityCurve(),
        };
        return newWallet;
      }

      const wallet = await api.addWallet({ address, label });
      return { ...wallet, equity_curve: generateEquityCurve() };
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.wallets.all });
    },
  });
}

export function useUntrackWalletMutation() {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (address: string) => {
      if (!isLiveMode) {
        await new Promise((resolve) => setTimeout(resolve, 400));
        return { success: true };
      }

      await api.deleteWallet(address);
      return { success: true };
    },
    onSuccess: (_, address) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.wallets.all });
      queryClient.removeQueries({
        queryKey: queryKeys.wallets.detail(address),
      });
    },
  });
}

export function useUpdateWalletMutation() {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';
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
      if (!isLiveMode) {
        await new Promise((resolve) => setTimeout(resolve, 400));
        return { address, ...params };
      }

      return api.updateWallet(address, params);
    },
    onSuccess: (_, { address }) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.wallets.all });
      queryClient.invalidateQueries({
        queryKey: queryKeys.wallets.detail(address),
      });
    },
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
