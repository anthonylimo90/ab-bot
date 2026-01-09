'use client';

import { useState, useEffect, useCallback } from 'react';
import { useWebSocket, ConnectionStatus } from './useWebSocket';
import { useModeStore } from '@/stores/mode-store';
import { api } from '@/lib/api';
import type { PortfolioStats, PortfolioHistory, WebSocketMessage, PositionUpdate } from '@/types/api';

interface UsePortfolioStatsReturn {
  stats: PortfolioStats;
  history: PortfolioHistory[];
  status: ConnectionStatus;
  isLoading: boolean;
  refresh: () => Promise<void>;
}

// Generate mock history data
function generateHistory(days: number, startValue: number): PortfolioHistory[] {
  const data: PortfolioHistory[] = [];
  let value = startValue * 0.9;
  const now = new Date();

  for (let i = days; i >= 0; i--) {
    const date = new Date(now);
    date.setDate(date.getDate() - i);

    // Random walk with slight upward bias
    const change = (Math.random() - 0.45) * 0.03 * value;
    value = Math.max(value + change, startValue * 0.7);

    data.push({
      timestamp: date.toISOString().split('T')[0],
      value: Math.round(value * 100) / 100,
    });
  }
  return data;
}

// Default stats
const defaultStats: PortfolioStats = {
  total_value: 10000,
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
  const { mode, demoBalance } = useModeStore();
  const isDemo = mode === 'demo';
  const isLiveMode = mode === 'live';

  const periodDays = { '1D': 1, '7D': 7, '30D': 30, 'ALL': 90 };

  const [stats, setStats] = useState<PortfolioStats>({
    ...defaultStats,
    total_value: isDemo ? demoBalance : 12345.67,
    total_pnl: 1234.56,
    total_pnl_percent: 12.3,
    today_pnl: 145.23,
    today_pnl_percent: 1.2,
    unrealized_pnl: 234.56,
    realized_pnl: 198.45,
    total_fees: -45.67,
    win_rate: 68,
    total_trades: 50,
    winning_trades: 34,
    active_positions: 12,
  });

  const [history, setHistory] = useState<PortfolioHistory[]>(() =>
    generateHistory(periodDays[period], isDemo ? demoBalance : 12345.67)
  );

  const [isLoading, setIsLoading] = useState(false);

  // Fetch stats from API (in live mode, compute from positions)
  const fetchStats = useCallback(async () => {
    if (!isLiveMode) {
      return;
    }

    try {
      setIsLoading(true);
      const positions = await api.getPositions({ status: 'open' });

      // Compute stats from positions
      const totalValue = positions.reduce((sum, p) => sum + p.quantity * p.current_price, 0);
      const unrealizedPnl = positions.reduce((sum, p) => sum + p.unrealized_pnl, 0);
      const copyTrades = positions.filter(p => p.is_copy_trade).length;

      setStats(prev => ({
        ...prev,
        total_value: totalValue || prev.total_value,
        unrealized_pnl: unrealizedPnl,
        active_positions: positions.length,
      }));

      // Generate history based on current value
      setHistory(generateHistory(periodDays[period], totalValue || 10000));
    } catch (err) {
      console.error('Failed to fetch portfolio stats:', err);
    } finally {
      setIsLoading(false);
    }
  }, [isLiveMode, period]);

  // Handle WebSocket position updates
  const handleMessage = useCallback((message: WebSocketMessage) => {
    if (message.type !== 'Position') return;

    const update = message.data as PositionUpdate;

    // Update stats based on position changes
    setStats(prev => ({
      ...prev,
      unrealized_pnl: prev.unrealized_pnl + (update.unrealized_pnl - prev.unrealized_pnl) / prev.active_positions,
    }));
  }, []);

  // WebSocket connection for live updates
  const { status } = useWebSocket({
    channel: 'positions',
    onMessage: handleMessage,
    enabled: isLiveMode,
  });

  // Initial fetch in live mode
  useEffect(() => {
    if (isLiveMode) {
      fetchStats();
    }
  }, [isLiveMode, fetchStats]);

  // Update history when period changes
  useEffect(() => {
    setIsLoading(true);
    const newHistory = generateHistory(periodDays[period], stats.total_value);
    setHistory(newHistory);
    setIsLoading(false);
  }, [period, stats.total_value]);

  // Demo mode: simulate real-time stats updates
  useEffect(() => {
    if (isLiveMode) return;

    const interval = setInterval(() => {
      setStats((prev) => {
        // Small random changes
        const todayChange = (Math.random() - 0.5) * 20;
        const newTodayPnl = prev.today_pnl + todayChange;
        const newTotalPnl = prev.total_pnl + todayChange;
        const newUnrealized = prev.unrealized_pnl + (Math.random() - 0.5) * 10;

        return {
          ...prev,
          today_pnl: Math.round(newTodayPnl * 100) / 100,
          today_pnl_percent: Math.round((newTodayPnl / prev.total_value) * 10000) / 100,
          total_pnl: Math.round(newTotalPnl * 100) / 100,
          total_pnl_percent: Math.round((newTotalPnl / (prev.total_value - newTotalPnl)) * 10000) / 100,
          unrealized_pnl: Math.round(newUnrealized * 100) / 100,
        };
      });

      // Add new point to history
      setHistory((prev) => {
        const lastValue = prev[prev.length - 1]?.value || stats.total_value;
        const change = (Math.random() - 0.48) * 0.005 * lastValue;
        const newValue = Math.round((lastValue + change) * 100) / 100;

        return [
          ...prev.slice(1),
          {
            timestamp: new Date().toISOString().split('T')[0],
            value: newValue,
          },
        ];
      });
    }, 3000);

    return () => clearInterval(interval);
  }, [isLiveMode, stats.total_value]);

  // Sync with demo balance
  useEffect(() => {
    if (isDemo) {
      setStats((prev) => ({
        ...prev,
        total_value: demoBalance,
      }));
    }
  }, [isDemo, demoBalance]);

  return {
    stats,
    history,
    status: isLiveMode ? status : 'connected',
    isLoading,
    refresh: fetchStats,
  };
}
