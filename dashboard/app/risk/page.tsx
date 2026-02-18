"use client";

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
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
import {
  useRiskStatusQuery,
  useManualTripMutation,
  useResetCircuitBreakerMutation,
} from "@/hooks/queries/useRiskQuery";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { api } from "@/lib/api";
import { formatCurrency, formatPercent, cn } from "@/lib/utils";
import {
  ShieldAlert,
  ShieldCheck,
  ShieldOff,
  Activity,
  Zap,
  Clock,
  AlertTriangle,
  type LucideIcon,
} from "lucide-react";
import type { TripReason, ServiceStatusItem } from "@/types/api";

const TRIP_REASON_LABELS: Record<TripReason, string> = {
  daily_loss_limit: "Daily Loss Limit",
  max_drawdown: "Max Drawdown",
  consecutive_losses: "Consecutive Losses",
  manual: "Manual Trip",
  connectivity: "Connectivity Issue",
  market_conditions: "Market Conditions",
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

function formatRelativeTime(dateStr: string): string {
  const diff = Date.now() - new Date(dateStr).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  const days = Math.floor(hrs / 24);
  return `${days}d ago`;
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

  const { data: serviceStatus } = useQuery({
    queryKey: ["service-status", workspaceId],
    queryFn: () => api.getServiceStatus(workspaceId!),
    enabled: Boolean(workspaceId),
    refetchInterval: 30000,
    staleTime: 15000,
  });

  const tripMutation = useManualTripMutation(workspaceId);
  const resetMutation = useResetCircuitBreakerMutation(workspaceId);

  const cb = riskStatus?.circuit_breaker;
  const sl = riskStatus?.stop_loss;

  const drawdownPct =
    cb && cb.peak_value > 0
      ? ((cb.peak_value - cb.current_value) / cb.peak_value) * 100
      : 0;

  return (
    <ErrorBoundary>
      <div className="space-y-6">
        {/* Header */}
        <div className="flex items-center justify-between">
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

        {/* Circuit Breaker Status Banner */}
        {cb && (
          <Card
            className={cn(
              "border-2",
              cb.tripped
                ? "border-loss bg-loss/5"
                : cb.recovery_state
                  ? "border-yellow-500 bg-yellow-500/5"
                  : "border-profit bg-profit/5",
            )}
          >
            <CardContent className="p-6">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-3">
                  {cb.tripped ? (
                    <ShieldOff className="h-8 w-8 text-loss" />
                  ) : cb.recovery_state ? (
                    <AlertTriangle className="h-8 w-8 text-yellow-500" />
                  ) : (
                    <ShieldCheck className="h-8 w-8 text-profit" />
                  )}
                  <div>
                    <div className="flex items-center gap-2">
                      <span
                        className={cn(
                          "text-lg font-bold",
                          cb.tripped
                            ? "text-loss"
                            : cb.recovery_state
                              ? "text-yellow-500"
                              : "text-profit",
                        )}
                      >
                        {cb.tripped
                          ? "CIRCUIT BREAKER TRIPPED"
                          : cb.recovery_state
                            ? "RECOVERY MODE"
                            : "OPERATIONAL"}
                      </span>
                      {cb.tripped && cb.trip_reason && (
                        <Badge variant="destructive" className="text-xs">
                          {TRIP_REASON_LABELS[cb.trip_reason]}
                        </Badge>
                      )}
                    </div>
                    {cb.tripped && cb.resume_at && (
                      <p className="text-sm text-muted-foreground">
                        Resumes in {formatTimeRemaining(cb.resume_at)}
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

                <div className="flex items-center gap-2">
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
                            This will immediately halt all trading activity. The
                            system will enter a cooldown period of{" "}
                            {cb.config.cooldown_minutes} minutes before
                            recovery.
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
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => resetMutation.mutate()}
                      disabled={resetMutation.isPending}
                    >
                      Reset
                    </Button>
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
            <MetricCard
              title="Drawdown"
              value={formatPercent(drawdownPct)}
              trend={drawdownPct > 0 ? "down" : "neutral"}
              changeLabel={`Max: ${formatPercent(cb.config.max_drawdown_pct * 100)}`}
            />
            <MetricCard
              title="Trips Today"
              value={String(cb.trips_today)}
              trend="neutral"
              changeLabel={`Cooldown: ${cb.config.cooldown_minutes}m`}
            />
          </div>
        )}

        {/* Stop-Loss Section */}
        {sl && (
          <div className="grid gap-6 lg:grid-cols-2">
            {/* Stats */}
            <Card>
              <CardHeader className="pb-3">
                <CardTitle className="flex items-center gap-2 text-base">
                  <Zap className="h-4 w-4" />
                  Stop-Loss Rules
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="grid grid-cols-2 gap-4">
                  <div>
                    <p className="text-sm text-muted-foreground">Active</p>
                    <p className="text-2xl font-bold tabular-nums">
                      {sl.active_rules}
                    </p>
                  </div>
                  <div>
                    <p className="text-sm text-muted-foreground">Executed</p>
                    <p className="text-2xl font-bold tabular-nums">
                      {sl.executed_rules}
                    </p>
                  </div>
                </div>
                <div className="flex flex-wrap gap-2">
                  {sl.fixed_stops > 0 && (
                    <Badge variant="secondary">
                      Fixed: {sl.fixed_stops}
                    </Badge>
                  )}
                  {sl.percentage_stops > 0 && (
                    <Badge variant="secondary">
                      Percentage: {sl.percentage_stops}
                    </Badge>
                  )}
                  {sl.trailing_stops > 0 && (
                    <Badge variant="secondary">
                      Trailing: {sl.trailing_stops}
                    </Badge>
                  )}
                  {sl.time_based_stops > 0 && (
                    <Badge variant="secondary">
                      Time-Based: {sl.time_based_stops}
                    </Badge>
                  )}
                  {sl.total_rules === 0 && (
                    <p className="text-sm text-muted-foreground">
                      No stop-loss rules configured
                    </p>
                  )}
                </div>
              </CardContent>
            </Card>

            {/* Recent Executions */}
            <Card>
              <CardHeader className="pb-3">
                <CardTitle className="flex items-center gap-2 text-base">
                  <Clock className="h-4 w-4" />
                  Recent Executions
                </CardTitle>
              </CardHeader>
              <CardContent>
                {sl.recent_executions.length === 0 ? (
                  <p className="text-sm text-muted-foreground py-4 text-center">
                    No stop-loss executions yet
                  </p>
                ) : (
                  <div className="space-y-2">
                    {sl.recent_executions.map((exec) => (
                      <div
                        key={exec.id}
                        className="flex items-center justify-between rounded-lg border px-3 py-2"
                      >
                        <div className="flex items-center gap-2">
                          <Badge variant="outline" className="text-xs">
                            {exec.stop_type}
                          </Badge>
                          <span className="text-sm font-mono text-muted-foreground truncate max-w-[160px]">
                            {exec.market_id}
                          </span>
                        </div>
                        <span className="text-xs text-muted-foreground whitespace-nowrap">
                          {formatRelativeTime(exec.executed_at)}
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </CardContent>
            </Card>
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
              Filters applied before arb entry signals are published.
            </p>
            <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
              <div className="rounded-lg border p-3">
                <p className="text-xs text-muted-foreground mb-1">
                  Min Net Profit
                </p>
                <p className="text-lg font-bold tabular-nums">0.5%</p>
                <p className="text-xs text-muted-foreground">
                  Must clear fees + slippage
                </p>
              </div>
              <div className="rounded-lg border p-3">
                <p className="text-xs text-muted-foreground mb-1">
                  Signal Cooldown
                </p>
                <p className="text-lg font-bold tabular-nums">60s</p>
                <p className="text-xs text-muted-foreground">
                  Per-market dedup window
                </p>
              </div>
              <div className="rounded-lg border p-3">
                <p className="text-xs text-muted-foreground mb-1">
                  Min Depth
                </p>
                <p className="text-lg font-bold tabular-nums">$100</p>
                <p className="text-xs text-muted-foreground">
                  Both sides at best ask
                </p>
              </div>
              <div className="rounded-lg border p-3">
                <p className="text-xs text-muted-foreground mb-1">
                  Trading Fee
                </p>
                <p className="text-lg font-bold tabular-nums">2.0%</p>
                <p className="text-xs text-muted-foreground">
                  Applied to notional cost
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
                  name="Copy Trading"
                  status={serviceStatus.copy_trading}
                />
                <ServicePill
                  name="Arb Executor"
                  status={serviceStatus.arb_executor}
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
