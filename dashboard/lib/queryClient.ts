import { QueryClient } from "@tanstack/react-query";

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
    all: () => ["positions"] as const,
    list: (filters?: { status?: string; market?: string }) =>
      [...queryKeys.positions.all(), "list", filters] as const,
    detail: (id: string) =>
      [...queryKeys.positions.all(), "detail", id] as const,
  },

  // Wallets
  wallets: {
    all: () => ["wallets"] as const,
    roster: () => [...queryKeys.wallets.all(), "roster"] as const,
    bench: () => [...queryKeys.wallets.all(), "bench"] as const,
    detail: (address: string) =>
      [...queryKeys.wallets.all(), "detail", address] as const,
    metrics: (address: string) =>
      [...queryKeys.wallets.all(), "metrics", address] as const,
  },

  // Markets
  markets: {
    all: ["markets"] as const,
    list: (filters?: { category?: string; active?: boolean }) =>
      [...queryKeys.markets.all, "list", filters] as const,
    detail: (id: string) => [...queryKeys.markets.all, "detail", id] as const,
    orderbook: (id: string) =>
      [...queryKeys.markets.all, "orderbook", id] as const,
  },

  // Discovery
  discover: {
    all: () => ["discover"] as const,
    byWorkspace: (workspaceId: string) =>
      [...queryKeys.discover.all(), "workspace", workspaceId] as const,
    wallets: (filters?: unknown, workspaceId?: string) =>
      [
        ...(workspaceId
          ? queryKeys.discover.byWorkspace(workspaceId)
          : queryKeys.discover.all()),
        "wallets",
        filters,
      ] as const,
    leaderboard: (workspaceId?: string) =>
      [
        ...(workspaceId
          ? queryKeys.discover.byWorkspace(workspaceId)
          : queryKeys.discover.all()),
        "leaderboard",
      ] as const,
    trades: (
      params?: { wallet?: string; limit?: number; minValue?: number },
      workspaceId?: string,
    ) =>
      [
        ...(workspaceId
          ? queryKeys.discover.byWorkspace(workspaceId)
          : queryKeys.discover.all()),
        "trades",
        params,
      ] as const,
  },

  // Portfolio
  portfolio: {
    all: () => ["portfolio"] as const,
    stats: () => [...queryKeys.portfolio.all(), "stats"] as const,
    history: (period: string) =>
      [...queryKeys.portfolio.all(), "history", period] as const,
  },

  // Backtest
  backtest: {
    all: ["backtest"] as const,
    results: () => [...queryKeys.backtest.all, "results"] as const,
    detail: (id: string) => [...queryKeys.backtest.all, "detail", id] as const,
  },

  // Optimizer
  optimizer: {
    all: ["optimizer"] as const,
    status: (workspaceId: string) =>
      [...queryKeys.optimizer.all, "status", workspaceId] as const,
  },

  // Allocations
  allocations: {
    all: () => ["allocations"] as const,
    byWorkspace: (workspaceId: string) =>
      [...queryKeys.allocations.all(), "workspace", workspaceId] as const,
    list: (workspaceId: string) =>
      [...queryKeys.allocations.byWorkspace(workspaceId), "list"] as const,
    active: (workspaceId: string) =>
      [...queryKeys.allocations.byWorkspace(workspaceId), "active"] as const,
    bench: (workspaceId: string) =>
      [...queryKeys.allocations.byWorkspace(workspaceId), "bench"] as const,
  },

  // Risk monitoring
  risk: {
    all: () => ["risk"] as const,
    status: (workspaceId: string) =>
      [...queryKeys.risk.all(), "status", workspaceId] as const,
  },

  dynamicTuning: {
    all: () => ["dynamic-tuning"] as const,
    status: (workspaceId: string) =>
      [...queryKeys.dynamicTuning.all(), "status", workspaceId] as const,
    history: (workspaceId: string, params?: { limit?: number; offset?: number }) =>
      [...queryKeys.dynamicTuning.all(), "history", workspaceId, params] as const,
  },

  // Rotation history
  rotationHistory: {
    all: () => ["rotation-history"] as const,
    byWorkspace: (workspaceId: string) =>
      [...queryKeys.rotationHistory.all(), "workspace", workspaceId] as const,
    list: (workspaceId: string, params?: { unacknowledgedOnly?: boolean }) =>
      [
        ...queryKeys.rotationHistory.byWorkspace(workspaceId),
        "list",
        params,
      ] as const,
  },
} as const;
