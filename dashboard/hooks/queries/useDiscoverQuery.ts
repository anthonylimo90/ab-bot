'use client';

import { useQuery } from '@tanstack/react-query';
import { useModeStore } from '@/stores/mode-store';
import { api } from '@/lib/api';
import { queryKeys } from '@/lib/queryClient';
import type { DiscoveredWallet, LiveTrade, DemoPnlSimulation } from '@/types/api';

// Mock discovered wallets for demo mode
const mockDiscoveredWallets: DiscoveredWallet[] = [
  {
    address: '0xnew1234567890abcdef1234567890abcdef1234',
    rank: 1,
    total_trades: 234,
    win_rate: 78,
    roi_7d: 12.4,
    roi_30d: 45.6,
    roi_90d: 156.4,
    sharpe_ratio: 2.8,
    total_pnl: 24500.0,
    max_drawdown: -8.2,
    prediction: 'HIGH_POTENTIAL',
    confidence: 0.92,
    is_tracked: false,
    trades_24h: 12,
  },
  {
    address: '0xnew9876543210fedcba9876543210fedcba9876',
    rank: 2,
    total_trades: 189,
    win_rate: 74,
    roi_7d: 8.2,
    roi_30d: 38.4,
    roi_90d: 128.2,
    sharpe_ratio: 2.4,
    total_pnl: 18300.0,
    max_drawdown: -12.5,
    prediction: 'HIGH_POTENTIAL',
    confidence: 0.85,
    is_tracked: false,
    trades_24h: 8,
  },
  {
    address: '0xnewabcdef1234567890abcdef1234567890abcd',
    rank: 3,
    total_trades: 312,
    win_rate: 71,
    roi_7d: 5.8,
    roi_30d: 28.2,
    roi_90d: 112.8,
    sharpe_ratio: 2.1,
    total_pnl: 15600.0,
    max_drawdown: -15.3,
    prediction: 'MODERATE',
    confidence: 0.78,
    is_tracked: true,
    trades_24h: 15,
  },
  {
    address: '0xnew567890abcdef1234567890abcdef12345678',
    rank: 4,
    total_trades: 156,
    win_rate: 69,
    roi_7d: 4.2,
    roi_30d: 22.5,
    roi_90d: 98.5,
    sharpe_ratio: 1.9,
    total_pnl: 12400.0,
    max_drawdown: -18.6,
    prediction: 'MODERATE',
    confidence: 0.72,
    is_tracked: false,
    trades_24h: 6,
  },
];

// Mock live trades for demo mode
const mockLiveTrades: LiveTrade[] = [
  {
    wallet_address: '0x1234567890abcdef1234567890abcdef12345678',
    wallet_label: 'Top Trader Alpha',
    tx_hash: '0xtx123456789abcdef',
    market_id: 'btc-100k-2024',
    market_question: 'Will BTC reach $100k by end of 2024?',
    outcome: 'yes',
    direction: 'buy',
    price: 0.72,
    quantity: 150,
    value: 108,
    timestamp: new Date(Date.now() - 120000).toISOString(),
  },
  {
    wallet_address: '0xabcdef1234567890abcdef1234567890abcdef12',
    wallet_label: 'Momentum Master',
    tx_hash: '0xtx987654321fedcba',
    market_id: 'eth-5k-2024',
    market_question: 'Will ETH reach $5k by end of 2024?',
    outcome: 'no',
    direction: 'sell',
    price: 0.38,
    quantity: 200,
    value: 76,
    timestamp: new Date(Date.now() - 300000).toISOString(),
  },
  {
    wallet_address: '0x9876543210fedcba9876543210fedcba98765432',
    wallet_label: 'Value Hunter',
    tx_hash: '0xtxabcdef123456789',
    market_id: 'trump-2024',
    market_question: 'Will Trump win 2024 election?',
    outcome: 'yes',
    direction: 'buy',
    price: 0.58,
    quantity: 300,
    value: 174,
    timestamp: new Date(Date.now() - 600000).toISOString(),
  },
];

interface DiscoverFilters {
  sortBy?: 'roi' | 'sharpe' | 'winRate' | 'trades';
  period?: '7d' | '30d' | '90d';
  minTrades?: number;
  minWinRate?: number;
  limit?: number;
}

export function useDiscoverWalletsQuery(filters?: DiscoverFilters) {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';

  return useQuery({
    queryKey: queryKeys.discover.wallets(filters),
    queryFn: async () => {
      if (!isLiveMode) {
        await new Promise((resolve) => setTimeout(resolve, 500));

        let wallets = [...mockDiscoveredWallets];

        // Apply sorting for demo
        if (filters?.sortBy) {
          wallets.sort((a, b) => {
            switch (filters.sortBy) {
              case 'roi':
                return b.roi_30d - a.roi_30d;
              case 'sharpe':
                return b.sharpe_ratio - a.sharpe_ratio;
              case 'winRate':
                return b.win_rate - a.win_rate;
              case 'trades':
                return b.total_trades - a.total_trades;
              default:
                return 0;
            }
          });
        }

        // Apply filters
        if (filters?.minTrades) {
          wallets = wallets.filter((w) => w.total_trades >= filters.minTrades!);
        }
        if (filters?.minWinRate) {
          wallets = wallets.filter((w) => w.win_rate >= filters.minWinRate!);
        }

        return wallets.slice(0, filters?.limit ?? 20);
      }

      return api.discoverWallets({
        sort_by: filters?.sortBy,
        period: filters?.period,
        min_trades: filters?.minTrades,
        min_win_rate: filters?.minWinRate,
        limit: filters?.limit,
      });
    },
    staleTime: 60 * 1000, // Discovery data refreshes every minute
  });
}

export function useLiveTradesQuery(params?: {
  wallet?: string;
  limit?: number;
  minValue?: number;
}) {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';

  return useQuery({
    queryKey: ['discover', 'trades', params],
    queryFn: async () => {
      if (!isLiveMode) {
        await new Promise((resolve) => setTimeout(resolve, 300));

        let trades = [...mockLiveTrades];

        if (params?.wallet) {
          trades = trades.filter(
            (t) =>
              t.wallet_address.toLowerCase() === params.wallet!.toLowerCase()
          );
        }
        if (params?.minValue) {
          trades = trades.filter((t) => t.value >= params.minValue!);
        }

        return trades.slice(0, params?.limit ?? 10);
      }

      return api.getLiveTrades({
        wallet: params?.wallet,
        limit: params?.limit,
        min_value: params?.minValue,
      });
    },
    staleTime: 10 * 1000, // Live trades refresh more frequently
    refetchInterval: isLiveMode ? 15 * 1000 : false,
  });
}

export function useDemoPnlSimulationQuery(params?: {
  amount?: number;
  period?: '7d' | '30d' | '90d';
  wallets?: string[];
}) {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';

  return useQuery({
    queryKey: ['discover', 'simulate', params],
    queryFn: async (): Promise<DemoPnlSimulation> => {
      if (!isLiveMode) {
        await new Promise((resolve) => setTimeout(resolve, 400));

        const amount = params?.amount ?? 1000;
        const multiplier =
          params?.period === '7d' ? 1.08 : params?.period === '90d' ? 1.35 : 1.18;

        const currentValue = Math.round(amount * multiplier * 100) / 100;
        const pnl = currentValue - amount;

        return {
          initial_amount: amount,
          current_value: currentValue,
          pnl: pnl,
          pnl_pct: (multiplier - 1) * 100,
          equity_curve: Array.from({ length: 30 }, (_, i) => ({
            date: new Date(Date.now() - (29 - i) * 24 * 60 * 60 * 1000)
              .toISOString()
              .split('T')[0],
            value:
              amount *
              (1 + ((multiplier - 1) * (i + 1)) / 30 + (Math.random() - 0.5) * 0.02),
          })),
          wallets: (params?.wallets ?? mockDiscoveredWallets.slice(0, 3).map(w => w.address)).map((address, i) => ({
            address,
            allocation_pct: Math.round(100 / (params?.wallets?.length ?? 3)),
            pnl: pnl / (params?.wallets?.length ?? 3),
            trades: 10 + i * 5,
          })),
        };
      }

      return api.simulateDemoPnl({
        amount: params?.amount,
        period: params?.period,
        wallets: params?.wallets?.join(','),
      });
    },
    staleTime: 5 * 60 * 1000, // Simulations don't change frequently
    enabled: true,
  });
}

export function useLeaderboardQuery() {
  return useDiscoverWalletsQuery({
    sortBy: 'roi',
    period: '30d',
    minTrades: 10,
    limit: 10,
  });
}
