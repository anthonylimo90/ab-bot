"use client";

import { createContext, useContext, useCallback, useRef, type ReactNode } from "react";
import { useWebSocket, type ConnectionStatus } from "@/hooks/useWebSocket";
import type { WebSocketMessage, PositionUpdate, SignalUpdate, OrderbookUpdate } from "@/types/api";

interface WebSocketContextValue {
  positionStatus: ConnectionStatus;
  latestPositionUpdate: PositionUpdate | null;
  signalStatus: ConnectionStatus;
  latestSignal: SignalUpdate | null;
  orderbookStatus: ConnectionStatus;
  orderbookUpdates: OrderbookUpdate[];
}

const WebSocketContext = createContext<WebSocketContextValue | null>(null);

export function useWebSocketContext() {
  const ctx = useContext(WebSocketContext);
  if (!ctx) {
    throw new Error("useWebSocketContext must be used within WebSocketProvider");
  }
  return ctx;
}

export function WebSocketProvider({ children }: { children: ReactNode }) {
  const latestPositionRef = useRef<PositionUpdate | null>(null);
  const latestSignalRef = useRef<SignalUpdate | null>(null);
  const orderbookRef = useRef<OrderbookUpdate[]>([]);

  const handlePositionMessage = useCallback((msg: WebSocketMessage) => {
    if (msg.type === "Position") {
      latestPositionRef.current = msg.data;
    }
  }, []);

  const handleSignalMessage = useCallback((msg: WebSocketMessage) => {
    if (msg.type === "Signal") {
      latestSignalRef.current = msg.data as SignalUpdate;
    }
  }, []);

  const handleOrderbookMessage = useCallback((msg: WebSocketMessage) => {
    if (msg.type === "Orderbook") {
      const updates = orderbookRef.current;
      // Keep last 10 updates
      updates.push(msg.data);
      if (updates.length > 10) updates.shift();
    }
  }, []);

  const { status: positionStatus } = useWebSocket({
    channel: "positions",
    onMessage: handlePositionMessage,
  });

  const { status: signalStatus } = useWebSocket({
    channel: "signals",
    onMessage: handleSignalMessage,
  });

  const { status: orderbookStatus } = useWebSocket({
    channel: "orderbook",
    onMessage: handleOrderbookMessage,
  });

  const value: WebSocketContextValue = {
    positionStatus,
    latestPositionUpdate: latestPositionRef.current,
    signalStatus,
    latestSignal: latestSignalRef.current,
    orderbookStatus,
    orderbookUpdates: orderbookRef.current,
  };

  return (
    <WebSocketContext.Provider value={value}>
      {children}
    </WebSocketContext.Provider>
  );
}
