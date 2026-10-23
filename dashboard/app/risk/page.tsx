"use client";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Progress } from "@/components/ui/progress";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { MetricCard } from "@/components/shared/MetricCard";
import { LiveIndicator } from "@/components/shared/LiveIndicator";
import { ErrorBoundary } from "@/components/shared/ErrorBoundary";
import { PageIntro } from "@/components/shared/PageIntro";
import {
  useRiskStatusQuery,
  useDynamicTunerQuery,
  useServiceStatusQuery,
  useManualTripMutation,
  useResetCircuitBreakerMutation,
} from "@/hooks/queries/useRiskQuery";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { formatCurrency, formatCurrencyOrDash, formatFixed, formatPercent, cn } from "@/lib/utils";
import {
  ShieldAlert,
  ShieldCheck,
  ShieldOff,
  Activity,
  Zap,
  AlertTriangle,
} from "lucide-react";
import type { TripReason, ServiceStatusItem } from "@/types/api";

const TRIP_REASON_LABELS: Record<TripReason, string> = {
  daily_loss_limit: "Daily Loss Limit",
  max_drawdown: "Max Drawdown",
  consecutive_losses: "Consecutive Losses",
  manual: "Manual Trip",
  connectivity: "Connectivity Issue",
  market_conditions: "Market Conditions",
  hard_kill_switch: "HARD KILL",
};

function formatTimeRemaining(resumeAt: string): string {
  const diff = new Date(resumeAt).getTime() - Date.now();
  if (diff <= 0) return "Resuming...";
  const mins = Math.ceil(diff / 60000);
  if (mins < 60) return `${mins}m`;
  const hrs = Math.floor(mins / 60);
  const remMins = mins % 60;
  return `${hrs}h ${remMins}m`;
}

function ServicePill({
  name,
  status,
}: {
  name: string;
  status: ServiceStatusItem;
}) {
  const pill = (
    <div
      className={cn(
        "flex items-center gap-2 rounded-full border px-3 py-1.5 text-sm font-medium",
        status.running
          ? "border-profit/20 bg-profit/10 text-profit"
          : "border-loss/20 bg-loss/10 text-loss",
      )}
    >
      <span
        className={cn(
          "h-2 w-2 rounded-full",
          status.running ? "bg-profit" : "bg-loss",
        )}
      />
      {name}
    </div>
  );

  if (status.reason && !status.running) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>{pill}</TooltipTrigger>
        <TooltipContent>
          <p className="text-xs">{status.reason}</p>
        </TooltipContent>
      </Tooltip>
    );
  }

  return pill;
}

export default function RiskPage() {
  const { currentWorkspace } = useWorkspaceStore();
  const workspaceId = currentWorkspace?.id;

  const { data: riskStatus, isLoading, error } = useRiskStatusQuery(workspaceId);
  const { data: serviceStatus } = useServiceStatusQuery(workspaceId);
  const { data: dynamicTunerStatus } = useDynamicTunerQuery(workspaceId);

  const tripMutation = useManualTripMutation(workspaceId);
  const resetMutation = useResetCircuitBreakerMutation(workspaceId);

  const cb = riskStatus?.circuit_breaker;

  // The trip measures from `drawdown_peak` (rebased on resume and reset);
  // the hard kill measures from the all-time `peak_value`.
  const drawdownFrom = (peak: number) =>
    cb && peak > 0 ? ((peak - cb.current_value) / peak) * 100 : 0;
  const drawdownPct = cb ? drawdownFrom(cb.drawdown_peak) : 0;
  const peakDrawdownPct = cb ? drawdownFrom(cb.peak_value) : 0;
  const breakerDisabled = cb ? !cb.config.enabled && !cb.tripped : false;

  return (
    <ErrorBoundary>
      <div className="space-y-5 sm:space-y-6">
        {/* Header */}
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center gap-3">
            <ShieldAlert className="h-6 w-6 text-muted-foreground" />
            <div>
              <h1 className="text-2xl font-bold">Risk Monitor</h1>
              <p className="text-sm text-muted-foreground">
                Real-time risk system activity
              </p>
            </div>
          </div>
          {cb && !cb.tripped && (
            <LiveIndicator />
          )}
          {cb?.tripped && (
            <div className="flex items-center gap-1.5">
              <span className="relative flex h-2 w-2">
                <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-loss opacity-75" />
                <span className="relative inline-flex rounded-full h-2 w-2 bg-loss" />
              </span>
              <span className="text-xs font-medium text-loss uppercase">
                HALTED
              </span>
            </div>
          )}
        </div>

        <PageIntro
          title="What this page means"
          description="This page tells you whether the system is safe to trade automatically, slowing itself down, or fully paused to avoid losses."
          bullets={[
            "If the circuit breaker is operational, automated trading is allowed to continue.",
            "If it is in recovery mode, the system is trading cautiously after a recent issue.",
            "If it is tripped, the system has halted automated trading and needs cooldown time or manual intervention."
          ]}
        />

        {/* Circuit Breaker Status Banner */}
        {cb && (
          <Card
            className={cn(
              "border-2",
              cb.tripped
                ? "border-loss bg-loss/5"
                : cb.recovery_state || breakerDisabled
                  ? "border-yellow-500 bg-yellow-500/5"
                  : "border-profit bg-profit/5",
            )}
          >
            <CardContent className="p-6">
              <div className="flex flex-col gap-4 lg:flex-row lg:items-center lg:justify-between">
                <div className="flex items-center gap-3">
                  {cb.trip_reason === "hard_kill_switch" ? (
                    <ShieldAlert className="h-8 w-8 text-loss animate-pulse" />
                  ) : cb.tripped ? (
                    <ShieldOff className="h-8 w-8 text-loss" />
                  ) : cb.recovery_state || breakerDisabled ? (
                    <AlertTriangle className="h-8 w-8 text-yellow-500" />
                  ) : (
                    <ShieldCheck className="h-8 w-8 text-profit" />
                  )}
                  <div>
                    <div className="flex flex-wrap items-center gap-2">
                      <span
                        className={cn(
                          "text-lg font-bold",
                          cb.trip_reason === "hard_kill_switch"
                            ? "text-loss"
                            : cb.tripped
                              ? "text-loss"
                              : cb.recovery_state || breakerDisabled
                                ? "text-yellow-500"
                                : "text-profit",
                        )}
                      >
                        {cb.trip_reason === "hard_kill_switch"
                          ? "HARD KILL \u2014 TRADING HALTED UNTIL RESET"
                          : cb.tripped
                            ? "CIRCUIT BREAKER TRIPPED"
                            : cb.recovery_state
                              ? "RECOVERY MODE"
                              : breakerDisabled
                                ? "DISABLED"
                                : "OPERATIONAL"}
                      </span>
                      {cb.tripped && cb.trip_reason && cb.trip_reason !== "hard_kill_switch" && (
                        <Badge variant="destructive" className="text-xs">
                          {TRIP_REASON_LABELS[cb.trip_reason]}
                        </Badge>
                      )}
                    </div>
                    {cb.trip_reason === "hard_kill_switch" && (
                      <p className="text-sm text-loss/80">
                        Drawdown exceeded the hard kill threshold. Manual reset required to resume trading.
                      </p>
                    )}
                    {cb.trip_reason === "manual" && (
                      <p className="text-sm text-muted-foreground">
                        Holds until an owner or admin resets it.
                      </p>
                    )}
                    {cb.tripped && cb.resume_at && (
                      <p className="text-sm text-muted-foreground">
                        Cooldown ends in {formatTimeRemaining(cb.resume_at)}
                      </p>
                    )}
                    {cb.trip_reason === "daily_loss_limit" && (
                      <p className="text-sm text-muted-foreground">
                        Holds while today&apos;s loss is over the limit; the
                        day&apos;s counters reset at 00:00 UTC.
                      </p>
                    )}
                    {breakerDisabled && (
                      <p className="text-sm text-muted-foreground">
                        Loss limits are not enforced. Manual and hard kill
                        trips still halt trading.
                      </p>
                    )}
                    {cb.recovery_state && (
                      <p className="text-sm text-muted-foreground">
                        Stage {cb.recovery_state.current_stage} of{" "}
                        {cb.recovery_state.total_stages} &mdash;{" "}
                        {(cb.recovery_state.capacity_pct * 100).toFixed(0)}%
                        capacity
                      </p>
                    )}
                  </div>
                </div>

                <div className="flex flex-wrap items-center gap-2">
                  {!cb.tripped && (
                    <AlertDialog>
                      <AlertDialogTrigger asChild>
                        <Button
                          variant="destructive"
                          size="sm"
                          disabled={tripMutation.isPending}
                        >
                          Manual Trip
                        </Button>
                      </AlertDialogTrigger>
                      <AlertDialogContent>
                        <AlertDialogHeader>
                          <AlertDialogTitle>
                            Trip Circuit Breaker?
                          </AlertDialogTitle>
                          <AlertDialogDescription>
                            This halts automated trading at once. A manual
                            trip does not expire: trading stays halted until
                            an owner or admin resets the breaker.
                          </AlertDialogDescription>
                        </AlertDialogHeader>
                        <AlertDialogFooter>
                          <AlertDialogCancel>Cancel</AlertDialogCancel>
                          <AlertDialogAction
                            onClick={() => tripMutation.mutate()}
                            className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                          >
                            Trip Circuit Breaker
                          </AlertDialogAction>
                        </AlertDialogFooter>
                      </AlertDialogContent>
                    </AlertDialog>
                  )}
                  {cb.tripped && (
                    <AlertDialog>
                      <AlertDialogTrigger asChild>
                        <Button
                          variant="outline"
                          size="sm"
                          disabled={resetMutation.isPending}
                        >
                          Reset
                        </Button>
                      </AlertDialogTrigger>
                      <AlertDialogContent>
                        <AlertDialogHeader>
                          <AlertDialogTitle>Reset Circuit Breaker?</AlertDialogTitle>
                          <AlertDialogDescription>
                            Trading resumes at once, ending this trip
                            {cb.trip_reason ? ` (${TRIP_REASON_LABELS[cb.trip_reason]})` : ""} and
                            its cooldown. Reset only once the cause is
                            understood.
                          </AlertDialogDescription>
                        </AlertDialogHeader>
                        <AlertDialogFooter>
                          <AlertDialogCancel>Cancel</AlertDialogCancel>
                          <AlertDialogAction onClick={() => resetMutation.mutate()}>
                            Reset and Resume Trading
                          </AlertDialogAction>
                        </AlertDialogFooter>
                      </AlertDialogContent>
                    </AlertDialog>
                  )}
                </div>
              </div>

              {/* Recovery progress bar */}
              {cb.recovery_state && (
                <div className="mt-4 space-y-1">
                  <Progress
                    value={cb.recovery_state.capacity_pct * 100}
                    className="h-2"
                  />
                  <div className="flex justify-between text-xs text-muted-foreground">
                    <span>
                      {cb.recovery_state.trades_this_stage} trades this stage
                    </span>
                    {cb.recovery_state.next_stage_at && (
                      <span>
                        Next stage in{" "}
                        {formatTimeRemaining(cb.recovery_state.next_stage_at)}
                      </span>
                    )}
                  </div>
                </div>
              )}
            </CardContent>
          </Card>
        )}

        {/* Metric Cards */}
        {cb && (
          <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
            <MetricCard
              title="Daily P&L"
              value={formatCurrency(cb.daily_pnl, { showSign: true })}
              trend={
                cb.daily_pnl > 0
                  ? "up"
                  : cb.daily_pnl < 0
                    ? "down"
                    : "neutral"
              }
              changeLabel={`Limit: ${formatCurrency(cb.config.max_daily_loss)}`}
            />
            <MetricCard
              title="Consecutive Losses"
              value={String(cb.consecutive_losses)}
              trend={cb.consecutive_losses > 0 ? "down" : "neutral"}
              changeLabel={`Limit: ${cb.config.max_consecutive_losses}`}
            />
            {cb.equity_seeded ? (
              <MetricCard
                title="Drawdown"
                value={formatPercent(drawdownPct)}
                trend={drawdownPct > 0 ? "down" : "neutral"}
                changeLabel={`Trip: ${formatPercent(cb.config.max_drawdown_pct * 100)}${cb.config.hard_kill_drawdown_pct > 0 ? ` | From peak: ${formatPercent(peakDrawdownPct)} of ${formatPercent(cb.config.hard_kill_drawdown_pct * 100)} kill` : ""}`}
              />
            ) : (
              <MetricCard
                title="Drawdown"
                value="Not checked"
                trend="neutral"
                changeLabel="No equity snapshot yet; drawdown limits are idle"
              />
            )}
            <MetricCard
              title="Trips Today"
              value={String(cb.trips_today)}
              trend="neutral"
              changeLabel={`Cooldown: ${cb.config.cooldown_minutes}m`}
            />
          </div>
        )}

        {/* Signal Quality Thresholds */}
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <Zap className="h-4 w-4" />
              Signal Quality Thresholds
            </CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-sm text-muted-foreground mb-4">
              Filters arb-monitor applies before it publishes an entry
              signal. A dash means the monitor is not reporting.
            </p>
            <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
              <div className="rounded-lg border p-3">
                <p className="text-xs text-muted-foreground mb-1">
                  Min Net Profit
                </p>
                <p className="text-lg font-bold tabular-nums">
                  {formatFixed(dynamicTunerStatus?.signal_thresholds.min_net_profit_threshold_pct, 2, "%")}
                </p>
                <p className="text-xs text-muted-foreground">
                  Must clear fees + slippage
                </p>
              </div>
              <div className="rounded-lg border p-3">
                <p className="text-xs text-muted-foreground mb-1">
                  Signal Cooldown
                </p>
                <p className="text-lg font-bold tabular-nums">
                  {formatFixed(dynamicTunerStatus?.signal_thresholds.signal_cooldown_secs, 0, "s")}
                </p>
                <p className="text-xs text-muted-foreground">
                  Per-market dedup window
                </p>
              </div>
              <div className="rounded-lg border p-3">
                <p className="text-xs text-muted-foreground mb-1">
                  Min Depth
                </p>
                <p className="text-lg font-bold tabular-nums">
                  {formatCurrencyOrDash(dynamicTunerStatus?.signal_thresholds.min_depth_usd)}
                </p>
                <p className="text-xs text-muted-foreground">
                  Each side, up to the entry price
                </p>
              </div>
              <div className="rounded-lg border p-3">
                <p className="text-xs text-muted-foreground mb-1">
                  Trading Fee
                </p>
                <p className="text-lg font-bold">Per market</p>
                <p className="text-xs text-muted-foreground">
                  Taker fee rate &times; p(1&minus;p) per share; 7% rate when a
                  market does not report its terms
                </p>
              </div>
            </div>
          </CardContent>
        </Card>

        {/* Service Health */}
        {serviceStatus && (
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="flex items-center gap-2 text-base">
                <Activity className="h-4 w-4" />
                Background Services
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex flex-wrap gap-3">
                <ServicePill
                  name="Harvester"
                  status={serviceStatus.harvester}
                />
                <ServicePill
                  name="Metrics"
                  status={serviceStatus.metrics_calculator}
                />
                <ServicePill
                  name="Arb Executor"
                  status={serviceStatus.arb_executor}
                />
                <ServicePill
                  name="Exit Handler"
                  status={serviceStatus.exit_handler}
                />
                <ServicePill
                  name="Live Trading"
                  status={serviceStatus.live_trading}
                />
              </div>
            </CardContent>
          </Card>
        )}

        {/* Loading state */}
        {isLoading && (
          <div className="flex items-center justify-center py-12">
            <p className="text-muted-foreground">Loading risk status...</p>
          </div>
        )}

        {/* Error state */}
        {error && !riskStatus && (
          <Card>
            <CardContent className="flex flex-col items-center justify-center py-12 text-center">
              <ShieldAlert className="h-12 w-12 text-muted-foreground mb-4" />
              <p className="text-lg font-medium">
                Failed to load risk status
              </p>
              <p className="text-sm text-muted-foreground mt-1">
                {error instanceof Error ? error.message : "Unknown error"}
              </p>
            </CardContent>
          </Card>
        )}
      </div>
    </ErrorBoundary>
  );
}
