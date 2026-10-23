"use client";

import {
  createElement,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useWebSocket, ConnectionStatus } from "./useWebSocket";
import { useToastStore } from "@/stores/toast-store";
import { api } from "@/lib/api";
import { signalToActivity } from "@/lib/activity";
import type {
  Activity,
  WebSocketMessage,
  SignalUpdate,
} from "@/types/api";

const SEEN_IDS_MAX = 1000;
const SEEN_IDS_TRIM_TO = 500;

function isSignalUpdate(data: unknown): data is SignalUpdate {
  if (typeof data !== "object" || data === null) return false;
  const d = data as Record<string, unknown>;
  return (
    typeof d.signal_id === "string" &&
    typeof d.signal_type === "string" &&
    typeof d.market_id === "string" &&
    typeof d.action === "string" &&
    typeof d.timestamp === "string"
  );
}

interface UseActivityReturn {
  activities: Activity[];
  status: ConnectionStatus;
  unreadCount: number;
  markAsRead: () => void;
}

const ActivityContext = createContext<UseActivityReturn | null>(null);

export function ActivityProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const [activities, setActivities] = useState<Activity[]>([]);
  const [unreadCount, setUnreadCount] = useState(0);
  const seenIds = useRef(new Set<string>());

  // Fetch persisted activity on mount
  useEffect(() => {
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
  }, []);

  // Handle WebSocket messages (signals)
  const handleMessage = useCallback((message: WebSocketMessage) => {
    if (message.type !== "Signal") return;
    if (!isSignalUpdate(message.data)) return;

    const activity = signalToActivity(message.data);

    // Deduplicate against REST-loaded activities
    if (seenIds.current.has(activity.id)) return;
    seenIds.current.add(activity.id);

    // Cap seenIds to prevent unbounded growth
    if (seenIds.current.size > SEEN_IDS_MAX) {
      const entries = Array.from(seenIds.current);
      seenIds.current = new Set(
        entries.slice(entries.length - SEEN_IDS_TRIM_TO),
      );
    }

    setActivities((prev) => [activity, ...prev].slice(0, 50));
    setUnreadCount((prev) => prev + 1);

    // Fire toast notification
    const toast = useToastStore.getState();
    switch (activity.type) {
      case "ARB_POSITION_OPENED":
        toast.success("Arb Position Opened", activity.message);
        break;
      case "ARB_POSITION_CLOSED":
        toast.info("Arb Position Closed", activity.message);
        break;
      case "ARB_EXECUTION_FAILED":
      case "ARB_EXIT_FAILED":
        toast.error("Trading Error", activity.message);
        break;
    }
  }, []);

  // WebSocket connection for live signals
  const { status } = useWebSocket({
    channel: "signals",
    onMessage: handleMessage,
  });

  const markAsRead = useCallback(() => {
    setUnreadCount(0);
  }, []);

  const value = useMemo(
    () => ({
      activities,
      status,
      unreadCount,
      markAsRead,
    }),
    [activities, status, unreadCount, markAsRead],
  );

  return createElement(ActivityContext.Provider, { value }, children);
}

export function useActivity(): UseActivityReturn {
  const context = useContext(ActivityContext);

  if (!context) {
    throw new Error("useActivity must be used within an ActivityProvider");
  }

  return context;
}
