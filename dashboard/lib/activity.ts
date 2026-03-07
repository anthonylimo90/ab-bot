import type { Activity } from "@/types/api";

function sourceOf(activity: Pick<Activity, "details">) {
  const source = activity.details?.source;
  return typeof source === "string" ? source : null;
}

export function isArbitrageActivity(
  activity: Pick<Activity, "type" | "details">,
) {
  return (
    activity.type.startsWith("ARB_") ||
    activity.type === "ARBITRAGE_DETECTED" ||
    sourceOf(activity) === "arbitrage"
  );
}

export function isRiskActivity(activity: Pick<Activity, "type" | "details">) {
  return (
    activity.type === "STOP_LOSS_TRIGGERED" ||
    activity.type === "TAKE_PROFIT_TRIGGERED" ||
    sourceOf(activity) === "stop_loss"
  );
}

export function isFailedActivity(
  activity: Pick<Activity, "type" | "details">,
) {
  return (
    activity.type === "ARB_EXECUTION_FAILED" ||
    activity.type === "ARB_EXIT_FAILED" ||
    activity.type === "TRADE_FAILED"
  );
}
