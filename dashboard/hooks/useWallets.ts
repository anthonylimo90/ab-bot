'use client';

import { useState, useEffect, useCallback } from 'react';
import { useModeStore } from '@/stores/mode-store';
import { api } from '@/lib/api';
import type { TrackedWallet, WalletMetrics, Wallet } from '@/types/api';

interface UseWalletsReturn {
  wallets: Wallet[];
  isLoading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  trackWallet: (address: string, label?: string) => Promise<void>;
  untrackWallet: (address: string) => Promise<void>;
  updateWallet: (address: string, params: { copy_enabled?: boolean; allocation_pct?: number }) => Promise<void>;
  getMetrics: (address: string) => Promise<WalletMetrics>;
}

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
    total_pnl: 15420.50,
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
    total_pnl: 5240.00,
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

export function useWallets(): UseWalletsReturn {
  const { mode } = useModeStore();
  const [wallets, setWallets] = useState<Wallet[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const isLiveMode = mode === 'live';

  // Fetch wallets from API
  const fetchWallets = useCallback(async () => {
    if (!isLiveMode) {
      setWallets(mockWallets);
      setIsLoading(false);
      return;
    }

    try {
      setIsLoading(true);
      setError(null);
      const data = await api.getWallets();

      // Enrich with metrics for each wallet
      const enrichedWallets: Wallet[] = await Promise.all(
        data.map(async (wallet) => {
          try {
            const metrics = await api.getWalletMetrics(wallet.address);
            return {
              ...wallet,
              metrics,
              equity_curve: generateEquityCurve(), // Would come from API in production
            };
          } catch {
            return { ...wallet, equity_curve: generateEquityCurve() };
          }
        })
      );

      setWallets(enrichedWallets);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch wallets');
      console.error('Failed to fetch wallets:', err);
    } finally {
      setIsLoading(false);
    }
  }, [isLiveMode]);

  // Initial fetch
  useEffect(() => {
    fetchWallets();
  }, [fetchWallets]);

  // Track a new wallet
  const trackWallet = useCallback(async (address: string, label?: string) => {
    if (!isLiveMode) {
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
      setWallets((prev) => [newWallet, ...prev]);
      return;
    }

    try {
      const wallet = await api.addWallet({ address, label });
      setWallets((prev) => [{ ...wallet, equity_curve: generateEquityCurve() }, ...prev]);
    } catch (err) {
      console.error('Failed to track wallet:', err);
      throw err;
    }
  }, [isLiveMode]);

  // Untrack a wallet
  const untrackWallet = useCallback(async (address: string) => {
    if (!isLiveMode) {
      setWallets((prev) => prev.filter((w) => w.address !== address));
      return;
    }

    try {
      await api.deleteWallet(address);
      setWallets((prev) => prev.filter((w) => w.address !== address));
    } catch (err) {
      console.error('Failed to untrack wallet:', err);
      throw err;
    }
  }, [isLiveMode]);

  // Update wallet settings
  const updateWallet = useCallback(async (
    address: string,
    params: { copy_enabled?: boolean; allocation_pct?: number }
  ) => {
    if (!isLiveMode) {
      setWallets((prev) =>
        prev.map((w) =>
          w.address === address ? { ...w, ...params } : w
        )
      );
      return;
    }

    try {
      const updated = await api.updateWallet(address, params);
      setWallets((prev) =>
        prev.map((w) =>
          w.address === address ? { ...w, ...updated } : w
        )
      );
    } catch (err) {
      console.error('Failed to update wallet:', err);
      throw err;
    }
  }, [isLiveMode]);

  // Get detailed metrics for a wallet
  const getMetrics = useCallback(async (address: string): Promise<WalletMetrics> => {
    if (!isLiveMode) {
      return {
        address,
        roi: 47.3,
        sharpe_ratio: 2.4,
        max_drawdown: -8.2,
        avg_trade_size: 150,
        avg_hold_time_hours: 24,
        profit_factor: 2.1,
        recent_pnl_30d: 1250.50,
        category_win_rates: { politics: 0.75, crypto: 0.68, sports: 0.62 },
        calculated_at: new Date().toISOString(),
      };
    }

    return api.getWalletMetrics(address);
  }, [isLiveMode]);

  return {
    wallets,
    isLoading,
    error,
    refresh: fetchWallets,
    trackWallet,
    untrackWallet,
    updateWallet,
    getMetrics,
  };
}
