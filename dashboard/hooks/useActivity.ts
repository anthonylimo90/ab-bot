'use client';

import { useState, useCallback } from 'react';
import { useWebSocket, ConnectionStatus } from './useWebSocket';
import type { Activity, ActivityType, WebSocketMessage, SignalUpdate } from '@/types/api';

interface UseActivityReturn {
  activities: Activity[];
  status: ConnectionStatus;
  unreadCount: number;
  markAsRead: () => void;
}

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
      if (signal.action === 'skipped') {
        type = 'TRADE_COPY_SKIPPED';
        message = `Skipped: ${signal.metadata?.reason || 'Unknown reason'}`;
      } else if (signal.action === 'failed') {
        type = 'TRADE_COPY_FAILED';
        message = `Failed: ${signal.metadata?.error || 'Unknown error'}`;
      } else if (signal.action === 'copied') {
        type = 'TRADE_COPIED';
        message = `Copied trade on ${signal.market_id}`;
      } else {
        type = 'TRADE_COPIED';
        message = `Copy trade signal: ${signal.action} on ${signal.market_id}`;
      }
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
  const [activities, setActivities] = useState<Activity[]>([]);
  const [unreadCount, setUnreadCount] = useState(0);

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
    enabled: true,
  });

  const markAsRead = useCallback(() => {
    setUnreadCount(0);
  }, []);

  return {
    activities,
    status,
    unreadCount,
    markAsRead,
  };
}
