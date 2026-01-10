'use client';

import { useState, useEffect, useCallback } from 'react';
import { useWebSocket, ConnectionStatus } from './useWebSocket';
import { api } from '@/lib/api';
import type { Position, PositionUpdate, WebSocketMessage } from '@/types/api';

interface UsePositionsReturn {
  positions: Position[];
  openPositions: Position[];
  closedPositions: Position[];
  status: ConnectionStatus;
  totalUnrealizedPnl: number;
  isLoading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  closePosition: (positionId: string) => Promise<void>;
}

export function usePositions(): UsePositionsReturn {
  const [positions, setPositions] = useState<Position[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Fetch positions from API
  const fetchPositions = useCallback(async () => {
    try {
      setIsLoading(true);
      setError(null);
      const data = await api.getPositions({ status: 'open' });
      setPositions(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch positions');
      console.error('Failed to fetch positions:', err);
    } finally {
      setIsLoading(false);
    }
  }, []);

  // Handle WebSocket position updates
  const handleMessage = useCallback((message: WebSocketMessage) => {
    if (message.type !== 'Position') return;

    const update = message.data as PositionUpdate;

    setPositions((prev) => {
      switch (update.update_type) {
        case 'Opened':
          // Fetch full position data
          api.getPosition(update.position_id).then((position) => {
            setPositions((current) => [position, ...current]);
          });
          return prev;

        case 'Updated':
        case 'PriceChanged':
          return prev.map((p) =>
            p.id === update.position_id
              ? {
                  ...p,
                  current_price: update.current_price,
                  unrealized_pnl: update.unrealized_pnl,
                  quantity: update.quantity,
                  updated_at: update.timestamp,
                }
              : p
          );

        case 'Closed':
          return prev.filter((p) => p.id !== update.position_id);

        default:
          return prev;
      }
    });
  }, []);

  // WebSocket connection for live updates
  const { status } = useWebSocket({
    channel: 'positions',
    onMessage: handleMessage,
    enabled: true,
  });

  // Initial fetch
  useEffect(() => {
    fetchPositions();
  }, [fetchPositions]);

  // Close position
  const closePosition = useCallback(async (positionId: string) => {
    try {
      await api.closePosition(positionId);
      setPositions((prev) => prev.filter((p) => p.id !== positionId));
    } catch (err) {
      console.error('Failed to close position:', err);
      throw err;
    }
  }, []);

  const openPositions = positions;
  const closedPositions: Position[] = []; // Would need separate API call
  const totalUnrealizedPnl = positions.reduce((sum, p) => sum + p.unrealized_pnl, 0);

  return {
    positions,
    openPositions,
    closedPositions,
    status,
    totalUnrealizedPnl,
    isLoading,
    error,
    refresh: fetchPositions,
    closePosition,
  };
}
