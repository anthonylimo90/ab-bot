'use client';

import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useModeStore } from '@/stores/mode-store';
import { api } from '@/lib/api';
import { queryKeys } from '@/lib/queryClient';
import type { Position, PositionStatus } from '@/types/api';

// Mock positions for demo mode
const mockPositions: Position[] = [
  {
    id: '1',
    market_id: 'btc-100k-2024',
    outcome: 'yes',
    side: 'long',
    quantity: 100,
    entry_price: 0.65,
    current_price: 0.72,
    stop_loss: 0.55,
    unrealized_pnl: 7.0,
    unrealized_pnl_pct: 10.77,
    is_copy_trade: true,
    source_wallet: '0x1234567890abcdef',
    opened_at: new Date(Date.now() - 3600000).toISOString(),
    updated_at: new Date().toISOString(),
  },
  {
    id: '2',
    market_id: 'eth-5k-2024',
    outcome: 'no',
    side: 'long',
    quantity: 50,
    entry_price: 0.4,
    current_price: 0.38,
    unrealized_pnl: 1.0,
    unrealized_pnl_pct: 5.0,
    is_copy_trade: false,
    opened_at: new Date(Date.now() - 7200000).toISOString(),
    updated_at: new Date().toISOString(),
  },
  {
    id: '3',
    market_id: 'trump-2024',
    outcome: 'yes',
    side: 'long',
    quantity: 200,
    entry_price: 0.52,
    current_price: 0.58,
    unrealized_pnl: 12.0,
    unrealized_pnl_pct: 11.54,
    is_copy_trade: false,
    opened_at: new Date(Date.now() - 86400000).toISOString(),
    updated_at: new Date().toISOString(),
  },
];

interface PositionFilters {
  status?: PositionStatus;
  market?: string;
  copyTradesOnly?: boolean;
}

export function usePositionsQuery(filters?: PositionFilters) {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';

  return useQuery({
    queryKey: queryKeys.positions.list(filters),
    queryFn: async () => {
      if (!isLiveMode) {
        // Simulate network delay for demo
        await new Promise((resolve) => setTimeout(resolve, 300));
        return mockPositions;
      }

      return api.getPositions({
        status: filters?.status,
        market_id: filters?.market,
        copy_trades_only: filters?.copyTradesOnly,
      });
    },
    staleTime: isLiveMode ? 30 * 1000 : 60 * 1000, // Demo data stays fresh longer
    refetchInterval: isLiveMode ? 30 * 1000 : false, // Only auto-refetch in live mode
  });
}

export function usePositionQuery(positionId: string) {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';

  return useQuery({
    queryKey: queryKeys.positions.detail(positionId),
    queryFn: async () => {
      if (!isLiveMode) {
        await new Promise((resolve) => setTimeout(resolve, 200));
        const position = mockPositions.find((p) => p.id === positionId);
        if (!position) throw new Error('Position not found');
        return position;
      }

      return api.getPosition(positionId);
    },
    enabled: !!positionId,
  });
}

export function useClosePositionMutation() {
  const { mode } = useModeStore();
  const isLiveMode = mode === 'live';
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      positionId,
      quantity,
      limitPrice,
    }: {
      positionId: string;
      quantity?: number;
      limitPrice?: number;
    }) => {
      if (!isLiveMode) {
        await new Promise((resolve) => setTimeout(resolve, 500));
        return { success: true, positionId };
      }

      return api.closePosition(positionId, {
        quantity,
        limit_price: limitPrice,
      });
    },
    onSuccess: (_, { positionId }) => {
      // Invalidate positions list
      queryClient.invalidateQueries({ queryKey: queryKeys.positions.all });
      // Remove specific position from cache
      queryClient.removeQueries({
        queryKey: queryKeys.positions.detail(positionId),
      });
    },
  });
}

// Derived hooks for common use cases
export function useOpenPositions() {
  const query = usePositionsQuery({ status: 'open' });

  const openPositions = query.data ?? [];
  const totalUnrealizedPnl = openPositions.reduce(
    (sum, p) => sum + p.unrealized_pnl,
    0
  );

  return {
    ...query,
    openPositions,
    totalUnrealizedPnl,
  };
}

export function useCopyTradePositions() {
  const query = usePositionsQuery({ copyTradesOnly: true });

  return {
    ...query,
    copyPositions: query.data ?? [],
  };
}
