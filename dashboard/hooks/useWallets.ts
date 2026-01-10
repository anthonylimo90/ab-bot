'use client';

import { useState, useEffect, useCallback } from 'react';
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

export function useWallets(): UseWalletsReturn {
  const [wallets, setWallets] = useState<Wallet[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Fetch wallets from API
  const fetchWallets = useCallback(async () => {
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
            };
          } catch {
            return wallet;
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
  }, []);

  // Initial fetch
  useEffect(() => {
    fetchWallets();
  }, [fetchWallets]);

  // Track a new wallet
  const trackWallet = useCallback(async (address: string, label?: string) => {
    try {
      const wallet = await api.addWallet({ address, label });
      setWallets((prev) => [wallet, ...prev]);
    } catch (err) {
      console.error('Failed to track wallet:', err);
      throw err;
    }
  }, []);

  // Untrack a wallet
  const untrackWallet = useCallback(async (address: string) => {
    try {
      await api.deleteWallet(address);
      setWallets((prev) => prev.filter((w) => w.address !== address));
    } catch (err) {
      console.error('Failed to untrack wallet:', err);
      throw err;
    }
  }, []);

  // Update wallet settings
  const updateWallet = useCallback(async (
    address: string,
    params: { copy_enabled?: boolean; allocation_pct?: number }
  ) => {
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
  }, []);

  // Get detailed metrics for a wallet
  const getMetrics = useCallback(async (address: string): Promise<WalletMetrics> => {
    return api.getWalletMetrics(address);
  }, []);

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
