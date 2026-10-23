import type { Activity, ActivityType, SignalUpdate } from "@/types/api";

type ActivityLike = {
  type: string;
  details?: Record<string, unknown>;
};

function sourceOf(activity: Pick<ActivityLike, "details">) {
  const source = activity.details?.source;
  return typeof source === "string" ? source : null;
}

export function isArbitrageActivity(activity: ActivityLike) {
  return (
    activity.type.startsWith("ARB_") ||
    activity.type === "ARBITRAGE_DETECTED" ||
    sourceOf(activity) === "arbitrage"
  );
}

export function isFailedActivity(activity: ActivityLike) {
  return (
    activity.type === "ARB_EXECUTION_FAILED" ||
    activity.type === "ARB_EXIT_FAILED" ||
    activity.type === "TRADE_FAILED"
  );
}

// Map signal type to activity type
export function signalToActivity(signal: SignalUpdate): Activity {
  let type: ActivityType = "ARBITRAGE_DETECTED";
  let message = "";

  // signal_type values are snake_case on the wire (websocket.rs SignalType).
  switch (signal.signal_type) {
    case "arbitrage": {
      switch (signal.action) {
        case "executed":
          type = "ARB_POSITION_OPENED";
          message = `Arb position opened on ${signal.market_id.slice(0, 20)}...`;
          break;
        case "execution_failed":
          type = "ARB_EXECUTION_FAILED";
          message = `Arb execution failed: ${signal.metadata?.reason || "Unknown"}`;
          break;
        case "closed_via_exit":
          type = "ARB_POSITION_CLOSED";
          message = `Arb position closed via exit`;
          break;
        case "closed_via_resolution":
          type = "ARB_POSITION_CLOSED";
          message = `Arb position closed via resolution`;
          break;
        case "exit":
          type = "ARB_POSITION_CLOSED";
          message = `Arb position exited`;
          break;
        default:
          type = "ARBITRAGE_DETECTED";
          message = `Arbitrage opportunity on ${signal.market_id.slice(0, 20)}...`;
          break;
      }
      break;
    }
    case "alert":
      if (signal.action === "exit_failed") {
        type = "ARB_EXIT_FAILED";
        message = `Exit failed: ${signal.metadata?.reason || "Unknown reason"}`;
      } else {
        type = "ARBITRAGE_DETECTED";
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
