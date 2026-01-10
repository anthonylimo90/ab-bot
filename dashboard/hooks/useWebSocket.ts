'use client';

import { useEffect, useRef, useState, useCallback } from 'react';
import type { WebSocketMessage } from '@/types/api';

export type ConnectionStatus =
  | 'connecting'
  | 'connected'
  | 'disconnected'
  | 'error';

const WS_BASE_URL = process.env.NEXT_PUBLIC_WS_URL || 'ws://localhost:3001';

export type WebSocketChannel = 'orderbook' | 'positions' | 'signals' | 'all';

interface WebSocketOptions {
  channel: WebSocketChannel;
  onMessage?: (data: WebSocketMessage) => void;
  onMessageBatch?: (data: WebSocketMessage[]) => void;
  onConnect?: () => void;
  onDisconnect?: () => void;
  onError?: (error: Event) => void;
  reconnect?: boolean;
  reconnectInterval?: number;
  maxReconnectAttempts?: number;
  enabled?: boolean;
  // Batching options
  batchMessages?: boolean;
  batchInterval?: number; // ms to wait before flushing batch
}

interface UseWebSocketReturn {
  status: ConnectionStatus;
  send: (data: Record<string, unknown>) => void;
  subscribe: (channel: string, filters?: Record<string, unknown>) => void;
  unsubscribe: (channel: string) => void;
  disconnect: () => void;
  reconnect: () => void;
}

export function useWebSocket(options: WebSocketOptions): UseWebSocketReturn {
  const {
    channel,
    onMessage,
    onMessageBatch,
    onConnect,
    onDisconnect,
    onError,
    reconnect = true,
    reconnectInterval = 3000,
    maxReconnectAttempts = 5,
    enabled = true,
    batchMessages = false,
    batchInterval = 100,
  } = options;

  const wsRef = useRef<WebSocket | null>(null);
  const reconnectAttemptsRef = useRef(0);
  const reconnectTimeoutRef = useRef<NodeJS.Timeout | null>(null);
  const pingIntervalRef = useRef<NodeJS.Timeout | null>(null);
  const [status, setStatus] = useState<ConnectionStatus>('disconnected');

  // Message batching refs
  const messageQueueRef = useRef<WebSocketMessage[]>([]);
  const flushTimeoutRef = useRef<NodeJS.Timeout | null>(null);

  const url = `${WS_BASE_URL}/ws/${channel}`;

  // Flush batched messages
  const flushBatch = useCallback(() => {
    if (messageQueueRef.current.length === 0) return;

    const messages = [...messageQueueRef.current];
    messageQueueRef.current = [];
    flushTimeoutRef.current = null;

    // Call batch handler if provided
    if (onMessageBatch) {
      onMessageBatch(messages);
    }

    // Also call individual handler for each message
    if (onMessage) {
      messages.forEach((msg) => onMessage(msg));
    }
  }, [onMessage, onMessageBatch]);

  // Queue message for batching
  const queueMessage = useCallback(
    (message: WebSocketMessage) => {
      messageQueueRef.current.push(message);

      // Schedule flush if not already scheduled
      if (!flushTimeoutRef.current) {
        flushTimeoutRef.current = setTimeout(flushBatch, batchInterval);
      }
    },
    [flushBatch, batchInterval]
  );

  // Process incoming message
  const handleMessage = useCallback(
    (message: WebSocketMessage) => {
      // Handle pong silently
      if (message.type === 'Pong') return;

      if (batchMessages) {
        queueMessage(message);
      } else {
        onMessage?.(message);
      }
    },
    [batchMessages, queueMessage, onMessage]
  );

  const connect = useCallback(() => {
    if (!enabled) return;
    if (wsRef.current?.readyState === WebSocket.OPEN) return;

    setStatus('connecting');

    try {
      const ws = new WebSocket(url);

      ws.onopen = () => {
        setStatus('connected');
        reconnectAttemptsRef.current = 0;
        onConnect?.();

        // Start ping interval to keep connection alive
        pingIntervalRef.current = setInterval(() => {
          if (ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ action: 'ping' }));
          }
        }, 30000);
      };

      ws.onmessage = (event) => {
        try {
          const data = JSON.parse(event.data) as WebSocketMessage;
          handleMessage(data);
        } catch {
          console.error('Failed to parse WebSocket message:', event.data);
        }
      };

      ws.onclose = () => {
        setStatus('disconnected');
        onDisconnect?.();

        // Clear ping interval
        if (pingIntervalRef.current) {
          clearInterval(pingIntervalRef.current);
        }

        // Flush any remaining batched messages
        if (flushTimeoutRef.current) {
          clearTimeout(flushTimeoutRef.current);
          flushBatch();
        }

        // Attempt reconnection
        if (
          reconnect &&
          reconnectAttemptsRef.current < maxReconnectAttempts &&
          enabled
        ) {
          reconnectAttemptsRef.current++;
          reconnectTimeoutRef.current = setTimeout(() => {
            connect();
          }, reconnectInterval);
        }
      };

      ws.onerror = (error) => {
        setStatus('error');
        onError?.(error);
      };

      wsRef.current = ws;
    } catch (error) {
      setStatus('error');
      console.error('WebSocket connection failed:', error);
    }
  }, [
    url,
    handleMessage,
    onConnect,
    onDisconnect,
    onError,
    reconnect,
    reconnectInterval,
    maxReconnectAttempts,
    enabled,
    flushBatch,
  ]);

  const disconnect = useCallback(() => {
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
    }
    if (pingIntervalRef.current) {
      clearInterval(pingIntervalRef.current);
    }
    if (flushTimeoutRef.current) {
      clearTimeout(flushTimeoutRef.current);
      flushBatch();
    }
    reconnectAttemptsRef.current = maxReconnectAttempts; // Prevent reconnection
    wsRef.current?.close();
  }, [maxReconnectAttempts, flushBatch]);

  const send = useCallback((data: Record<string, unknown>) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(data));
    }
  }, []);

  const subscribe = useCallback(
    (channelName: string, filters?: Record<string, unknown>) => {
      send({
        action: 'subscribe',
        channel: channelName,
        filters,
      });
    },
    [send]
  );

  const unsubscribe = useCallback(
    (channelName: string) => {
      send({
        action: 'unsubscribe',
        channel: channelName,
      });
    },
    [send]
  );

  const manualReconnect = useCallback(() => {
    reconnectAttemptsRef.current = 0;
    disconnect();
    setTimeout(connect, 100);
  }, [connect, disconnect]);

  useEffect(() => {
    if (enabled) {
      connect();
    }

    return () => {
      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current);
      }
      if (pingIntervalRef.current) {
        clearInterval(pingIntervalRef.current);
      }
      if (flushTimeoutRef.current) {
        clearTimeout(flushTimeoutRef.current);
      }
      wsRef.current?.close();
    };
  }, [connect, enabled]);

  return {
    status,
    send,
    subscribe,
    unsubscribe,
    disconnect,
    reconnect: manualReconnect,
  };
}

/**
 * Hook for batched position updates - processes multiple updates efficiently
 */
interface PositionUpdateAccumulator {
  updates: Map<string, { price: number; pnl: number; quantity: number }>;
  opened: string[];
  closed: string[];
}

export function useBatchedPositionUpdates(
  onUpdate: (accumulator: PositionUpdateAccumulator) => void,
  debounceMs = 200
) {
  const accumulatorRef = useRef<PositionUpdateAccumulator>({
    updates: new Map(),
    opened: [],
    closed: [],
  });
  const timeoutRef = useRef<NodeJS.Timeout | null>(null);

  const flush = useCallback(() => {
    if (
      accumulatorRef.current.updates.size === 0 &&
      accumulatorRef.current.opened.length === 0 &&
      accumulatorRef.current.closed.length === 0
    ) {
      return;
    }

    onUpdate(accumulatorRef.current);

    // Reset accumulator
    accumulatorRef.current = {
      updates: new Map(),
      opened: [],
      closed: [],
    };
    timeoutRef.current = null;
  }, [onUpdate]);

  const addUpdate = useCallback(
    (
      positionId: string,
      updateType: 'Opened' | 'Updated' | 'Closed' | 'PriceChanged',
      data?: { price?: number; pnl?: number; quantity?: number }
    ) => {
      switch (updateType) {
        case 'Opened':
          accumulatorRef.current.opened.push(positionId);
          break;
        case 'Closed':
          accumulatorRef.current.closed.push(positionId);
          // Remove from updates if pending
          accumulatorRef.current.updates.delete(positionId);
          break;
        case 'Updated':
        case 'PriceChanged':
          if (data) {
            const existing = accumulatorRef.current.updates.get(positionId);
            accumulatorRef.current.updates.set(positionId, {
              price: data.price ?? existing?.price ?? 0,
              pnl: data.pnl ?? existing?.pnl ?? 0,
              quantity: data.quantity ?? existing?.quantity ?? 0,
            });
          }
          break;
      }

      // Schedule flush if not already scheduled
      if (!timeoutRef.current) {
        timeoutRef.current = setTimeout(flush, debounceMs);
      }
    },
    [flush, debounceMs]
  );

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }
    };
  }, []);

  return { addUpdate, flush };
}
