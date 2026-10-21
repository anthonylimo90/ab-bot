'use client';

import { useEffect, useMemo, useRef, useState, useCallback } from 'react';
import type { WebSocketMessage } from '@/types/api';
import { useAuthStore } from '@/stores/auth-store';
import { normalizeWsMessage } from '@/lib/wire';
import {
  createPositionBatcher,
  type PositionUpdateAccumulator,
} from '@/lib/position-batcher';

export type ConnectionStatus =
  | 'connecting'
  | 'connected'
  | 'disconnected'
  | 'error';

const WS_BASE_URL = process.env.NEXT_PUBLIC_WS_URL || 'ws://localhost:3001';

// Wire drift (a renamed variant, an unreadable price) would otherwise fail
// silently, so say so once per message type instead of once per frame.
const warnedMessageTypes = new Set<string>();
function warnUnrecognisedMessage(parsed: unknown) {
  const type =
    typeof parsed === 'object' && parsed !== null && 'type' in parsed
      ? String((parsed as { type: unknown }).type)
      : 'unknown';
  if (warnedMessageTypes.has(type)) return;
  warnedMessageTypes.add(type);
  console.warn(`Ignoring unrecognised or malformed WebSocket message (type: ${type})`);
}

export type WebSocketChannel =
  | 'orderbook'
  | 'positions'
  | 'signals'
  | 'trade-flow'
  | 'all';

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

  const token = useAuthStore((state) => state.token);
  const privateChannel = channel !== 'orderbook';
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectAttemptsRef = useRef(0);
  const reconnectTimeoutRef = useRef<NodeJS.Timeout | null>(null);
  const pingIntervalRef = useRef<NodeJS.Timeout | null>(null);
  const [status, setStatus] = useState<ConnectionStatus>('disconnected');

  // Message batching refs
  const messageQueueRef = useRef<WebSocketMessage[]>([]);
  const flushTimeoutRef = useRef<NodeJS.Timeout | null>(null);

  // Refs for latest callbacks to avoid stale closures
  const onMessageRef = useRef(onMessage);
  const onMessageBatchRef = useRef(onMessageBatch);
  const onConnectRef = useRef(onConnect);
  const onDisconnectRef = useRef(onDisconnect);
  const onErrorRef = useRef(onError);

  // Keep refs up to date
  useEffect(() => { onMessageRef.current = onMessage; }, [onMessage]);
  useEffect(() => { onMessageBatchRef.current = onMessageBatch; }, [onMessageBatch]);
  useEffect(() => { onConnectRef.current = onConnect; }, [onConnect]);
  useEffect(() => { onDisconnectRef.current = onDisconnect; }, [onDisconnect]);
  useEffect(() => { onErrorRef.current = onError; }, [onError]);

  const url = `${WS_BASE_URL}/ws/${channel}`;

  // Flush batched messages - uses refs to avoid stale closures
  const flushBatch = useCallback(() => {
    if (messageQueueRef.current.length === 0) return;

    const messages = [...messageQueueRef.current];
    messageQueueRef.current = [];
    flushTimeoutRef.current = null;

    if (onMessageBatchRef.current) {
      onMessageBatchRef.current(messages);
    }

    if (onMessageRef.current) {
      messages.forEach((msg) => onMessageRef.current!(msg));
    }
  }, []);

  // Queue message for batching
  const queueMessage = useCallback(
    (message: WebSocketMessage) => {
      messageQueueRef.current.push(message);

      if (!flushTimeoutRef.current) {
        flushTimeoutRef.current = setTimeout(flushBatch, batchInterval);
      }
    },
    [flushBatch, batchInterval]
  );

  // Process incoming message - uses refs for latest callbacks
  const handleMessage = useCallback(
    (message: WebSocketMessage) => {
      if (message.type === 'Pong') return;

      if (batchMessages) {
        queueMessage(message);
      } else {
        onMessageRef.current?.(message);
      }
    },
    [batchMessages, queueMessage]
  );

  const connect = useCallback(() => {
    if (!enabled || (privateChannel && !token)) {
      setStatus('disconnected');
      return;
    }
    if (wsRef.current?.readyState === WebSocket.OPEN) return;

    setStatus('connecting');

    try {
      const ws = new WebSocket(url);

      ws.onopen = () => {
        if (privateChannel) {
          ws.send(JSON.stringify({ action: 'authenticate', token }));
        }
        pingIntervalRef.current = setInterval(() => {
          if (ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ action: 'ping' }));
          }
        }, 30000);
      };

      ws.onmessage = (event) => {
        let parsed: unknown;
        try {
          parsed = JSON.parse(event.data);
        } catch {
          console.error('Failed to parse WebSocket message:', event.data);
          return;
        }
        // Validate the frame and convert Rust Decimal strings to numbers before any
        // consumer (query cache, components) sees it.
        const data = normalizeWsMessage(parsed);
        if (!data) {
          warnUnrecognisedMessage(parsed);
          return;
        }
        if (data.type === 'Subscribed') {
          setStatus('connected');
          reconnectAttemptsRef.current = 0;
          onConnectRef.current?.();
        }
        handleMessage(data);
      };

      ws.onclose = (event) => {
        setStatus(event.code === 1008 ? 'error' : 'disconnected');
        onDisconnectRef.current?.();

        if (pingIntervalRef.current) {
          clearInterval(pingIntervalRef.current);
        }

        if (flushTimeoutRef.current) {
          clearTimeout(flushTimeoutRef.current);
          flushBatch();
        }

        if (
          event.code !== 1008 &&
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
        onErrorRef.current?.(error);
      };

      wsRef.current = ws;
    } catch (error) {
      setStatus('error');
      console.error('WebSocket connection failed:', error);
    }
  }, [
    url,
    token,
    privateChannel,
    handleMessage,
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

    // Reconnect when tab becomes visible again
    const handleVisibilityChange = () => {
      if (
        document.visibilityState === 'visible' &&
        enabled &&
        wsRef.current?.readyState !== WebSocket.OPEN
      ) {
        reconnectAttemptsRef.current = 0;
        connect();
      }
    };
    document.addEventListener('visibilitychange', handleVisibilityChange);

    return () => {
      document.removeEventListener('visibilitychange', handleVisibilityChange);
      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current);
      }
      if (pingIntervalRef.current) {
        clearInterval(pingIntervalRef.current);
      }
      if (flushTimeoutRef.current) {
        clearTimeout(flushTimeoutRef.current);
      }
      if (wsRef.current) wsRef.current.onclose = null;
      wsRef.current?.close();
      messageQueueRef.current = [];
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
export function useBatchedPositionUpdates(
  onUpdate: (accumulator: PositionUpdateAccumulator) => void,
  debounceMs = 200
) {
  // The batcher outlives renders; route delivery through a ref so it always calls
  // the latest callback without being recreated.
  const onUpdateRef = useRef(onUpdate);
  useEffect(() => {
    onUpdateRef.current = onUpdate;
  }, [onUpdate]);

  const batcher = useMemo(
    () => createPositionBatcher((accumulator) => onUpdateRef.current(accumulator), debounceMs),
    [debounceMs]
  );

  // Cleanup on unmount
  useEffect(() => () => batcher.dispose(), [batcher]);

  return { addUpdate: batcher.addUpdate, flush: batcher.flush };
}
