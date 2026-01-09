'use client';

import { useState, useEffect, useCallback } from 'react';
import { useWebSocket, ConnectionStatus } from './useWebSocket';
import { useModeStore } from '@/stores/mode-store';
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
    entry_price: 0.40,
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

export function usePositions(): UsePositionsReturn {
  const { mode } = useModeStore();
  const [positions, setPositions] = useState<Position[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const isLiveMode = mode === 'live';

  // Fetch positions from API
  const fetchPositions = useCallback(async () => {
    if (!isLiveMode) {
      setPositions(mockPositions);
      setIsLoading(false);
      return;
    }

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
  }, [isLiveMode]);

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
    enabled: isLiveMode,
  });

  // Demo mode: simulate price updates
  useEffect(() => {
    if (isLiveMode) return;

    const interval = setInterval(() => {
      setPositions((prev) =>
        prev.map((p) => {
          // Random price movement
          const change = (Math.random() - 0.5) * 0.02;
          const newPrice = Math.max(0.01, Math.min(0.99, p.current_price + change));
          const priceDiff = p.side === 'long'
            ? newPrice - p.entry_price
            : p.entry_price - newPrice;
          const pnl = priceDiff * p.quantity;
          const pnlPct = (priceDiff / p.entry_price) * 100;

          return {
            ...p,
            current_price: Math.round(newPrice * 100) / 100,
            unrealized_pnl: Math.round(pnl * 100) / 100,
            unrealized_pnl_pct: Math.round(pnlPct * 100) / 100,
            updated_at: new Date().toISOString(),
          };
        })
      );
    }, 2000);

    return () => clearInterval(interval);
  }, [isLiveMode]);

  // Initial fetch
  useEffect(() => {
    fetchPositions();
  }, [fetchPositions]);

  // Close position
  const closePosition = useCallback(async (positionId: string) => {
    if (!isLiveMode) {
      setPositions((prev) => prev.filter((p) => p.id !== positionId));
      return;
    }

    try {
      await api.closePosition(positionId);
      setPositions((prev) => prev.filter((p) => p.id !== positionId));
    } catch (err) {
      console.error('Failed to close position:', err);
      throw err;
    }
  }, [isLiveMode]);

  const openPositions = positions;
  const closedPositions: Position[] = []; // Would need separate API call
  const totalUnrealizedPnl = positions.reduce((sum, p) => sum + p.unrealized_pnl, 0);

  return {
    positions,
    openPositions,
    closedPositions,
    status: isLiveMode ? status : 'connected',
    totalUnrealizedPnl,
    isLoading,
    error,
    refresh: fetchPositions,
    closePosition,
  };
}
