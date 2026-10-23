"use client";

import { useQuery, useMutation, useQueryClient, keepPreviousData } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import { useToastStore } from "@/stores/toast-store";
import type { Position, PositionStatus, PositionsSummary } from "@/types/api";

interface PositionFilters {
  status?: PositionStatus;
  market?: string;
  limit?: number;
  offset?: number;
}

export function usePositionsQuery(filters?: PositionFilters, options: {enabled?: boolean; keepPreviousData?: boolean} = {}) {
  return useQuery({
    queryKey: queryKeys.positions.list(filters),
    queryFn: () =>
      api.getPositions({
        status: filters?.status,
        market_id: filters?.market,
        limit: filters?.limit,
        offset: filters?.offset,
      }),
    enabled: options.enabled ?? true,
    placeholderData: options.keepPreviousData ? keepPreviousData : undefined,
    staleTime: 30 * 1000,
    refetchInterval: filters?.status === "closed" ? 60_000 : 30_000,
  });
}

export function usePositionsSummaryQuery() {
  return useQuery<PositionsSummary>({
    queryKey: queryKeys.positions.summary(),
    queryFn: () => api.getPositionsSummary(),
    staleTime: 15 * 1000,
    refetchInterval: 30 * 1000,
  });
}

export function usePositionQuery(positionId: string) {
  return useQuery({
    queryKey: queryKeys.positions.detail(positionId),
    queryFn: () => api.getPosition(positionId),
    enabled: Boolean(positionId),
  });
}

export function useClosePositionMutation() {
  const queryClient = useQueryClient();
  const toast = useToastStore();

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
      queryClient.invalidateQueries({ queryKey: queryKeys.positions.all() });
      queryClient.invalidateQueries({
        queryKey: queryKeys.positions.detail(positionId),
      });
      toast.success(
        "Exit requested",
        "The position has been handed to the canonical exit flow and will move through exit-ready or closing states before it is fully closed.",
      );
    },
    onError: (error: Error) => {
      toast.error("Exit not requested", error.message);
    },
  });
}

// Derived hooks for common use cases
export function useOpenPositions() {
  const query = usePositionsQuery({ status: "open", limit: 500 });

  const openPositions = query.data ?? [];
  const totalUnrealizedPnl = openPositions.reduce(
    (sum, p) => sum + p.unrealized_pnl,
    0,
  );

  return {
    ...query,
    openPositions,
    totalUnrealizedPnl,
  };
}
