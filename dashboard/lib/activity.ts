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

export function isRiskActivity(activity: ActivityLike) {
  return (
    activity.type === "STOP_LOSS_TRIGGERED" ||
    activity.type === "TAKE_PROFIT_TRIGGERED" ||
    sourceOf(activity) === "stop_loss"
  );
}

export function isFailedActivity(activity: ActivityLike) {
  return (
    activity.type === "ARB_EXECUTION_FAILED" ||
    activity.type === "ARB_EXIT_FAILED" ||
    activity.type === "TRADE_FAILED"
  );
}
