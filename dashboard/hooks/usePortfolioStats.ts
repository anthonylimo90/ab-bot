'use client';

import { useState, useEffect, useCallback } from 'react';
import { useWebSocket, ConnectionStatus } from './useWebSocket';
import { api } from '@/lib/api';
import type { PortfolioStats, PortfolioHistory, WebSocketMessage, PositionUpdate } from '@/types/api';

interface UsePortfolioStatsReturn {
  stats: PortfolioStats;
  history: PortfolioHistory[];
  status: ConnectionStatus;
  isLoading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
}

// Default stats
const defaultStats: PortfolioStats = {
  total_value: 0,
  total_pnl: 0,
  total_pnl_percent: 0,
  today_pnl: 0,
  today_pnl_percent: 0,
  unrealized_pnl: 0,
  realized_pnl: 0,
  total_fees: 0,
  win_rate: 0,
  total_trades: 0,
  winning_trades: 0,
  active_positions: 0,
};

export function usePortfolioStats(period: '1D' | '7D' | '30D' | 'ALL' = '30D'): UsePortfolioStatsReturn {
  const [stats, setStats] = useState<PortfolioStats>(defaultStats);
  const [history, setHistory] = useState<PortfolioHistory[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Fetch stats from API
  const fetchStats = useCallback(async () => {
    try {
      setIsLoading(true);
      setError(null);
      const positions = await api.getPositions({ status: 'open' });

      // Compute stats from positions
      const totalValue = positions.reduce((sum, p) => sum + p.quantity * p.current_price, 0);
      const unrealizedPnl = positions.reduce((sum, p) => sum + p.unrealized_pnl, 0);

      setStats(prev => ({
        ...prev,
        total_value: totalValue,
        unrealized_pnl: unrealizedPnl,
        active_positions: positions.length,
      }));
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch portfolio stats');
      console.error('Failed to fetch portfolio stats:', err);
    } finally {
      setIsLoading(false);
    }
  }, []);

  // Handle WebSocket position updates
  const handleMessage = useCallback((message: WebSocketMessage) => {
    if (message.type !== 'Position') return;

    const update = message.data as PositionUpdate;

    // Refresh stats on position changes
    if (update.update_type === 'Opened' || update.update_type === 'Closed') {
      fetchStats();
    } else {
      // Update unrealized PnL for price changes
      setStats(prev => ({
        ...prev,
        unrealized_pnl: prev.unrealized_pnl + update.unrealized_pnl,
      }));
    }
  }, [fetchStats]);

  // WebSocket connection for live updates
  const { status } = useWebSocket({
    channel: 'positions',
    onMessage: handleMessage,
    enabled: true,
  });

  // Initial fetch
  useEffect(() => {
    fetchStats();
  }, [fetchStats]);

  return {
    stats,
    history,
    status,
    isLoading,
    error,
    refresh: fetchStats,
  };
}
