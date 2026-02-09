import { QueryClient } from '@tanstack/react-query';
import type { TradingMode } from '@/stores/mode-store';

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Data considered fresh for 30 seconds
      staleTime: 30 * 1000,
      // Cache data for 5 minutes
      gcTime: 5 * 60 * 1000,
      // Refetch when window regains focus
      refetchOnWindowFocus: true,
      // Retry failed requests twice
      retry: 2,
      // Exponential backoff for retries
      retryDelay: (attemptIndex) => Math.min(1000 * 2 ** attemptIndex, 30000),
    },
    mutations: {
      // Retry mutations once
      retry: 1,
    },
  },
});

// Query key factory for consistent key management
export const queryKeys = {
  // Positions
  positions: {
    all: (mode: TradingMode) => ['positions', mode] as const,
    list: (mode: TradingMode, filters?: { status?: string; market?: string }) =>
      [...queryKeys.positions.all(mode), 'list', filters] as const,
    detail: (mode: TradingMode, id: string) => [...queryKeys.positions.all(mode), 'detail', id] as const,
  },

  // Wallets
  wallets: {
    all: (mode: TradingMode) => ['wallets', mode] as const,
    roster: (mode: TradingMode) => [...queryKeys.wallets.all(mode), 'roster'] as const,
    bench: (mode: TradingMode) => [...queryKeys.wallets.all(mode), 'bench'] as const,
    detail: (mode: TradingMode, address: string) =>
      [...queryKeys.wallets.all(mode), 'detail', address] as const,
    metrics: (mode: TradingMode, address: string) =>
      [...queryKeys.wallets.all(mode), 'metrics', address] as const,
  },

  // Markets
  markets: {
    all: ['markets'] as const,
    list: (filters?: { category?: string; active?: boolean }) =>
      [...queryKeys.markets.all, 'list', filters] as const,
    detail: (id: string) => [...queryKeys.markets.all, 'detail', id] as const,
    orderbook: (id: string) =>
      [...queryKeys.markets.all, 'orderbook', id] as const,
  },

  // Discovery
  discover: {
    all: (mode: TradingMode) => ['discover', mode] as const,
    byWorkspace: (mode: TradingMode, workspaceId: string) =>
      [...queryKeys.discover.all(mode), 'workspace', workspaceId] as const,
    wallets: (mode: TradingMode, filters?: unknown, workspaceId?: string) => [
      ...(workspaceId ? queryKeys.discover.byWorkspace(mode, workspaceId) : queryKeys.discover.all(mode)),
      'wallets',
      filters,
    ] as const,
    leaderboard: (mode: TradingMode, workspaceId?: string) => [
      ...(workspaceId ? queryKeys.discover.byWorkspace(mode, workspaceId) : queryKeys.discover.all(mode)),
      'leaderboard',
    ] as const,
    trades: (mode: TradingMode, params?: { wallet?: string; limit?: number; minValue?: number }, workspaceId?: string) => [
      ...(workspaceId ? queryKeys.discover.byWorkspace(mode, workspaceId) : queryKeys.discover.all(mode)),
      'trades',
      params,
    ] as const,
    simulate: (mode: TradingMode, params?: { amount?: number; period?: string; wallets?: string[] }, workspaceId?: string) => [
      ...(workspaceId ? queryKeys.discover.byWorkspace(mode, workspaceId) : queryKeys.discover.all(mode)),
      'simulate',
      params,
    ] as const,
  },

  // Portfolio
  portfolio: {
    all: (mode: TradingMode) => ['portfolio', mode] as const,
    stats: (mode: TradingMode) => [...queryKeys.portfolio.all(mode), 'stats'] as const,
    history: (mode: TradingMode, period: string) =>
      [...queryKeys.portfolio.all(mode), 'history', period] as const,
  },

  // Backtest
  backtest: {
    all: ['backtest'] as const,
    results: () => [...queryKeys.backtest.all, 'results'] as const,
    detail: (id: string) => [...queryKeys.backtest.all, 'detail', id] as const,
  },

  // Optimizer
  optimizer: {
    all: ['optimizer'] as const,
    status: (workspaceId: string) =>
      [...queryKeys.optimizer.all, 'status', workspaceId] as const,
  },

  // Allocations
  allocations: {
    all: (mode: TradingMode) => ['allocations', mode] as const,
    byWorkspace: (mode: TradingMode, workspaceId: string) =>
      [...queryKeys.allocations.all(mode), 'workspace', workspaceId] as const,
    list: (mode: TradingMode, workspaceId: string) =>
      [...queryKeys.allocations.byWorkspace(mode, workspaceId), 'list'] as const,
    active: (mode: TradingMode, workspaceId: string) =>
      [...queryKeys.allocations.byWorkspace(mode, workspaceId), 'active'] as const,
    bench: (mode: TradingMode, workspaceId: string) =>
      [...queryKeys.allocations.byWorkspace(mode, workspaceId), 'bench'] as const,
  },

  // Rotation history
  rotationHistory: {
    all: (mode: TradingMode) => ['rotation-history', mode] as const,
    byWorkspace: (mode: TradingMode, workspaceId: string) =>
      [...queryKeys.rotationHistory.all(mode), 'workspace', workspaceId] as const,
    list: (mode: TradingMode, workspaceId: string, params?: { unacknowledgedOnly?: boolean }) =>
      [...queryKeys.rotationHistory.byWorkspace(mode, workspaceId), 'list', params] as const,
  },
} as const;
