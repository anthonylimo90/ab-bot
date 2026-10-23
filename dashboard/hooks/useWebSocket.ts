'use client';

import { useEffect, useMemo, useRef, useState, useCallback } from 'react';
import type { WebSocketMessage } from '@/types/api';
import { useAuthStore } from '@/stores/auth-store';
import { normalizeWsMessage } from '@/lib/wire';
import {
  createReconnectingSocket,
  type ReconnectingSocket,
} from '@/lib/reconnecting-socket';
import {
  createPositionBatcher,
  type PositionUpdateAccumulator,
} from '@/lib/position-batcher';

export type ConnectionStatus =
  | 'connecting'
  | 'connected'
  | 'disconnected'
  | 'error';

const WS_BASE_URL = process.env.NEXT_PUBLIC_WS_URL || 'ws://localhost:3000';

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
    // Reconnect indefinitely by default (jittered exponential backoff capped at 30s).
    maxReconnectAttempts = Infinity,
    enabled = true,
    batchMessages = false,
    batchInterval = 100,
  } = options;

  const token = useAuthStore((state) => state.token);
  const privateChannel = channel !== 'orderbook';
  const socketRef = useRef<ReconnectingSocket | null>(null);
  const [status, setStatus] = useState<ConnectionStatus>('disconnected');

  // Message batching refs
  const messageQueueRef = useRef<WebSocketMessage[]>([]);
  const flushTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

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
    // Always release the timer handle, even when the queue is empty; a stale handle
    // would stop queueMessage from ever scheduling another flush.
    if (flushTimeoutRef.current) {
      clearTimeout(flushTimeoutRef.current);
      flushTimeoutRef.current = null;
    }
    if (messageQueueRef.current.length === 0) return;

    const messages = [...messageQueueRef.current];
    messageQueueRef.current = [];

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
  const handleMessageRef = useRef(handleMessage);
  useEffect(() => { handleMessageRef.current = handleMessage; }, [handleMessage]);

  // One ReconnectingSocket per (url, token, enabled) combination. The connection
  // itself (connect guard, backoff, pings, replacing dead sockets) lives in
  // lib/reconnecting-socket.ts; this effect only wires it to React state.
  useEffect(() => {
    if (!enabled || (privateChannel && !token)) {
      setStatus('disconnected');
      return;
    }

    const socket = createReconnectingSocket({
      url,
      authToken: privateChannel ? token : null,
      reconnect,
      baseDelayMs: reconnectInterval,
      maxAttempts: maxReconnectAttempts,
      onStatus: setStatus,
      onMessage: (raw) => {
        let parsed: unknown;
        try {
          parsed = JSON.parse(raw as string);
        } catch {
          console.error('Failed to parse WebSocket message:', raw);
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
          // The server confirmed the subscription: the connection is established
          // and the reconnect backoff starts over.
          socket.markConnected();
          onConnectRef.current?.();
        }
        handleMessageRef.current(data);
      },
      onDisconnect: () => {
        onDisconnectRef.current?.();
        // Deliver whatever was queued before the connection dropped.
        flushBatch();
      },
      onError: (event) => onErrorRef.current?.(event as Event),
    });
    socketRef.current = socket;
    socket.connect();

    // Reconnect promptly when the tab becomes visible again.
    const handleVisibilityChange = () => {
      if (document.visibilityState === 'visible') {
        socket.resume();
      }
    };
    document.addEventListener('visibilitychange', handleVisibilityChange);

    return () => {
      document.removeEventListener('visibilitychange', handleVisibilityChange);
      socket.dispose();
      if (socketRef.current === socket) socketRef.current = null;
      // Drop (do not deliver) anything still queued, and release the timer handle
      // so a later effect run on this hook instance can schedule flushes again.
      if (flushTimeoutRef.current) {
        clearTimeout(flushTimeoutRef.current);
        flushTimeoutRef.current = null;
      }
      messageQueueRef.current = [];
    };
  }, [
    url,
    token,
    privateChannel,
    enabled,
    reconnect,
    reconnectInterval,
    maxReconnectAttempts,
    flushBatch,
  ]);

  const disconnect = useCallback(() => {
    socketRef.current?.disconnect();
    flushBatch();
  }, [flushBatch]);

  const send = useCallback((data: Record<string, unknown>) => {
    socketRef.current?.send(JSON.stringify(data));
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
    socketRef.current?.reconnectNow();
  }, []);

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
