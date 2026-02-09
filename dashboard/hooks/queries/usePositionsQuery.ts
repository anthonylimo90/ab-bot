'use client';

import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { api } from '@/lib/api';
import { queryKeys } from '@/lib/queryClient';
import type { Position, PositionStatus } from '@/types/api';
import type { TradingMode } from '@/stores/mode-store';

interface PositionFilters {
  status?: PositionStatus;
  market?: string;
  copyTradesOnly?: boolean;
}

export function usePositionsQuery(mode: TradingMode, filters?: PositionFilters) {
  return useQuery({
    queryKey: queryKeys.positions.list(mode, filters),
    queryFn: () =>
      api.getPositions({
        status: filters?.status,
        market_id: filters?.market,
        copy_trades_only: filters?.copyTradesOnly,
      }),
    enabled: mode === 'live',
    staleTime: 30 * 1000,
    refetchInterval: 30 * 1000,
  });
}

export function usePositionQuery(mode: TradingMode, positionId: string) {
  return useQuery({
    queryKey: queryKeys.positions.detail(mode, positionId),
    queryFn: () => api.getPosition(positionId),
    enabled: !!positionId && mode === 'live',
  });
}

export function useClosePositionMutation(mode: TradingMode) {
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
      return api.closePosition(positionId, {
        quantity,
        limit_price: limitPrice,
      });
    },
    onSuccess: (_, { positionId }) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.positions.all(mode) });
      queryClient.removeQueries({
        queryKey: queryKeys.positions.detail(mode, positionId),
      });
    },
  });
}

// Derived hooks for common use cases
export function useOpenPositions(mode: TradingMode) {
  const query = usePositionsQuery(mode, { status: 'open' });

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

export function useCopyTradePositions(mode: TradingMode) {
  const query = usePositionsQuery(mode, { copyTradesOnly: true });

  return {
    ...query,
    copyPositions: query.data ?? [],
  };
}
