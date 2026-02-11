'use client';

import { useState, useCallback, useEffect, useRef } from 'react';
import { useWebSocket, ConnectionStatus } from './useWebSocket';
import { useModeStore } from '@/stores/mode-store';
import { useToastStore } from '@/stores/toast-store';
import { api } from '@/lib/api';
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
    case 'Arbitrage': {
      switch (signal.action) {
        case 'executed':
          type = 'ARB_POSITION_OPENED';
          message = `Arb position opened on ${signal.market_id.slice(0, 20)}...`;
          break;
        case 'execution_failed':
          type = 'ARB_EXECUTION_FAILED';
          message = `Arb execution failed: ${signal.metadata?.reason || 'Unknown'}`;
          break;
        case 'closed_via_exit':
          type = 'ARB_POSITION_CLOSED';
          message = `Arb position closed via exit`;
          break;
        case 'closed_via_resolution':
          type = 'ARB_POSITION_CLOSED';
          message = `Arb position closed via resolution`;
          break;
        case 'exit':
          type = 'ARB_POSITION_CLOSED';
          message = `Arb position exited`;
          break;
        default:
          type = 'ARBITRAGE_DETECTED';
          message = `Arbitrage opportunity on ${signal.market_id.slice(0, 20)}...`;
          break;
      }
      break;
    }
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
      if (signal.action === 'exit_failed') {
        type = 'ARB_EXIT_FAILED';
        message = `Exit failed: ${signal.metadata?.reason || 'Unknown reason'}`;
      } else {
        type = 'RECOMMENDATION_NEW';
        message = String(signal.metadata?.message || signal.action);
      }
      break;
  }

  const pnl = signal.metadata?.realized_pnl
    ? parseFloat(String(signal.metadata.realized_pnl))
    : signal.metadata?.profit
      ? parseFloat(String(signal.metadata.profit))
      : undefined;

  return {
    id: signal.signal_id,
    type,
    message,
    details: signal.metadata,
    pnl,
    created_at: signal.timestamp,
  };
}

export function useActivity(): UseActivityReturn {
  const { mode } = useModeStore();
  const [activities, setActivities] = useState<Activity[]>([]);
  const [unreadCount, setUnreadCount] = useState(0);
  const seenIds = useRef(new Set<string>());

  // Fetch persisted activity on mount (live mode only)
  useEffect(() => {
    if (mode !== 'live') {
      setActivities([]);
      seenIds.current.clear();
      return;
    }

    let cancelled = false;
    api
      .getActivity({ limit: 50 })
      .then((data) => {
        if (cancelled) return;
        // Track IDs for deduplication
        for (const item of data) {
          seenIds.current.add(item.id);
        }
        setActivities(data);
      })
      .catch(() => {
        // Silently fail — WebSocket will still work
      });

    return () => {
      cancelled = true;
    };
  }, [mode]);

  // Handle WebSocket messages (signals)
  const handleMessage = useCallback((message: WebSocketMessage) => {
    if (message.type !== 'Signal') return;

    const activity = signalToActivity(message.data as SignalUpdate);

    // Deduplicate against REST-loaded activities
    if (seenIds.current.has(activity.id)) return;
    seenIds.current.add(activity.id);

    setActivities((prev) => [activity, ...prev].slice(0, 50));
    setUnreadCount((prev) => prev + 1);

    // Fire toast notification
    const toast = useToastStore.getState();
    switch (activity.type) {
      case 'TRADE_COPIED':
        toast.success('Trade Copied', activity.message);
        break;
      case 'ARB_POSITION_OPENED':
        toast.success('Arb Position Opened', activity.message);
        break;
      case 'ARB_POSITION_CLOSED':
        toast.info('Arb Position Closed', activity.message);
        break;
      case 'TRADE_COPY_FAILED':
      case 'ARB_EXECUTION_FAILED':
      case 'ARB_EXIT_FAILED':
        toast.error('Trading Error', activity.message);
        break;
      case 'TRADE_COPY_SKIPPED':
        toast.warning('Trade Skipped', activity.message);
        break;
      case 'STOP_LOSS_TRIGGERED':
      case 'TAKE_PROFIT_TRIGGERED':
        toast.warning('Risk Alert', activity.message);
        break;
    }
  }, []);

  // WebSocket connection for live signals (only in live mode)
  const { status } = useWebSocket({
    channel: 'signals',
    onMessage: handleMessage,
    enabled: mode === 'live',
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
