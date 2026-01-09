'use client';

import { useState, useEffect, useCallback } from 'react';
import { useWebSocket, ConnectionStatus } from './useWebSocket';
import { useModeStore } from '@/stores/mode-store';
import type { Activity, ActivityType, WebSocketMessage, SignalUpdate } from '@/types/api';

interface UseActivityReturn {
  activities: Activity[];
  status: ConnectionStatus;
  unreadCount: number;
  markAsRead: () => void;
}

// Mock initial activities
const mockActivities: Activity[] = [
  {
    id: '1',
    type: 'TRADE_COPIED',
    message: 'Copied trade from 0x1234...5678',
    details: { market: 'BTC $100k YES', amount: 100 },
    pnl: 12.5,
    created_at: new Date(Date.now() - 5 * 60 * 1000).toISOString(),
  },
  {
    id: '2',
    type: 'STOP_LOSS_TRIGGERED',
    message: 'Stop-loss triggered on ETH market',
    pnl: -8.2,
    created_at: new Date(Date.now() - 15 * 60 * 1000).toISOString(),
  },
  {
    id: '3',
    type: 'RECOMMENDATION_NEW',
    message: 'New recommendation: Copy wallet 0xABC...DEF',
    details: { confidence: 85, roi: 34 },
    created_at: new Date(Date.now() - 30 * 60 * 1000).toISOString(),
  },
  {
    id: '4',
    type: 'ARBITRAGE_DETECTED',
    message: 'Arbitrage opportunity detected',
    details: { spread: 2.3 },
    created_at: new Date(Date.now() - 45 * 60 * 1000).toISOString(),
  },
];

// Activity generators for simulation
const activityGenerators: (() => Omit<Activity, 'id' | 'created_at'>)[] = [
  () => ({
    type: 'TRADE_COPIED' as ActivityType,
    message: `Copied trade from 0x${Math.random().toString(16).slice(2, 6)}...`,
    details: { market: ['BTC $100k', 'ETH $5k', 'SOL $500'][Math.floor(Math.random() * 3)] },
    pnl: Math.round((Math.random() - 0.3) * 30 * 100) / 100,
  }),
  () => ({
    type: 'ARBITRAGE_DETECTED' as ActivityType,
    message: 'Arbitrage opportunity detected',
    details: { spread: Math.round(Math.random() * 5 * 10) / 10 },
  }),
  () => ({
    type: 'POSITION_OPENED' as ActivityType,
    message: `Opened position on ${['BTC', 'ETH', 'SOL'][Math.floor(Math.random() * 3)]} market`,
    details: { side: Math.random() > 0.5 ? 'YES' : 'NO' },
  }),
];

// Map signal type to activity type
function signalToActivity(signal: SignalUpdate): Activity {
  let type: ActivityType = 'RECOMMENDATION_NEW';
  let message = '';

  switch (signal.signal_type) {
    case 'Arbitrage':
      type = 'ARBITRAGE_DETECTED';
      message = `Arbitrage opportunity on ${signal.market_id}`;
      break;
    case 'CopyTrade':
      type = 'TRADE_COPIED';
      message = `Copy trade signal: ${signal.action} on ${signal.market_id}`;
      break;
    case 'StopLoss':
      type = 'STOP_LOSS_TRIGGERED';
      message = `Stop-loss triggered on ${signal.market_id}`;
      break;
    case 'TakeProfit':
      type = 'TAKE_PROFIT_TRIGGERED';
      message = `Take-profit triggered on ${signal.market_id}`;
      break;
    case 'Alert':
      type = 'RECOMMENDATION_NEW';
      message = signal.action;
      break;
  }

  return {
    id: signal.signal_id,
    type,
    message,
    details: signal.metadata,
    created_at: signal.timestamp,
  };
}

export function useActivity(): UseActivityReturn {
  const { mode } = useModeStore();
  const [activities, setActivities] = useState<Activity[]>(mockActivities);
  const [unreadCount, setUnreadCount] = useState(0);

  const isLiveMode = mode === 'live';

  // Handle WebSocket messages (signals)
  const handleMessage = useCallback((message: WebSocketMessage) => {
    if (message.type !== 'Signal') return;

    const activity = signalToActivity(message.data as SignalUpdate);
    setActivities((prev) => [activity, ...prev].slice(0, 50));
    setUnreadCount((prev) => prev + 1);
  }, []);

  // WebSocket connection for live signals
  const { status } = useWebSocket({
    channel: 'signals',
    onMessage: handleMessage,
    enabled: isLiveMode,
  });

  // Simulate random activity for demo mode
  useEffect(() => {
    if (isLiveMode) return;

    const interval = setInterval(() => {
      // 20% chance of new activity every 5 seconds
      if (Math.random() < 0.2) {
        const generator = activityGenerators[Math.floor(Math.random() * activityGenerators.length)];
        const newActivity: Activity = {
          ...generator(),
          id: Math.random().toString(36).slice(2),
          created_at: new Date().toISOString(),
        };

        setActivities((prev) => [newActivity, ...prev].slice(0, 50));
        setUnreadCount((prev) => prev + 1);
      }
    }, 5000);

    return () => clearInterval(interval);
  }, [isLiveMode]);

  const markAsRead = useCallback(() => {
    setUnreadCount(0);
  }, []);

  return {
    activities,
    status: isLiveMode ? status : 'connected',
    unreadCount,
    markAsRead,
  };
}
