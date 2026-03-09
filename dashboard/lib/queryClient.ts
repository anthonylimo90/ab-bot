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
    list: (filters?: {
      status?: string;
      market?: string;
      limit?: number;
      offset?: number;
    }) =>
      [...queryKeys.positions.all(), "list", filters] as const,
    summary: () => [...queryKeys.positions.all(), "summary"] as const,
    detail: (id: string) =>
      [...queryKeys.positions.all(), "detail", id] as const,
  },

  account: {
    all: () => ["account"] as const,
    summary: (workspaceId: string) =>
      [...queryKeys.account.all(), "summary", workspaceId] as const,
    history: (workspaceId: string, params?: { hours?: number; limit?: number }) =>
      [...queryKeys.account.all(), "history", workspaceId, params] as const,
    cashFlows: (workspaceId: string, params?: { limit?: number }) =>
      [...queryKeys.account.all(), "cash-flows", workspaceId, params] as const,
  },

  // Wallets
  wallets: {
    all: () => ["wallets"] as const,
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

  // Backtest
  backtest: {
    all: ["backtest"] as const,
    results: () => [...queryKeys.backtest.all, "results"] as const,
    detail: (id: string) => [...queryKeys.backtest.all, "detail", id] as const,
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

  tradeFlow: {
    all: () => ["trade-flow"] as const,
    summary: (params?: { from?: string; to?: string; strategy?: string; limit?: number }) =>
      [...queryKeys.tradeFlow.all(), "summary", params] as const,
    journeys: (params?: { from?: string; to?: string; strategy?: string; limit?: number }) =>
      [...queryKeys.tradeFlow.all(), "journeys", params] as const,
    arbExecutionTelemetry: (params?: { from?: string; to?: string; limit?: number }) =>
      [...queryKeys.tradeFlow.all(), "arb-execution-telemetry", params] as const,
    learningOverview: (params?: { from?: string; to?: string; limit?: number }) =>
      [...queryKeys.tradeFlow.all(), "learning-overview", params] as const,
    learningRolloutDetail: (rolloutId: string, params?: { limit?: number }) =>
      [...queryKeys.tradeFlow.all(), "learning-rollout-detail", rolloutId, params] as const,
    market: (
      marketId: string,
      params?: { from?: string; to?: string; strategy?: string; limit?: number },
    ) => [...queryKeys.tradeFlow.all(), "market", marketId, params] as const,
  },

  // Orders
  orders: {
    all: () => ["orders"] as const,
    detail: (id: string) => [...queryKeys.orders.all(), "detail", id] as const,
  },

  // Quant signals
  signals: {
    all: () => ["signals"] as const,
    flow: (conditionId: string, windowMinutes?: number) =>
      [...queryKeys.signals.all(), "flow", conditionId, windowMinutes] as const,
    recent: (params?: { kind?: string; limit?: number }) =>
      [...queryKeys.signals.all(), "recent", params] as const,
    performance: (periodDays?: number) =>
      [...queryKeys.signals.all(), "performance", periodDays] as const,
    metadata: (params?: { category?: string; active?: boolean; limit?: number }) =>
      [...queryKeys.signals.all(), "metadata", params] as const,
  },
} as const;
