"use client";

import { useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { InfoTooltip } from "@/components/shared/InfoTooltip";
import { LiveIndicator } from "@/components/shared/LiveIndicator";
import { MetricCard } from "@/components/shared/MetricCard";
import { PageIntro } from "@/components/shared/PageIntro";
import { useArbExecutionTelemetryQuery } from "@/hooks/queries/useTradeFlowQuery";
import { cn } from "@/lib/utils";
import { Gauge, AlertTriangle, TimerReset, Activity } from "lucide-react";

type WindowKey = "24h" | "7d";

const WINDOWS: Record<WindowKey, { label: string; hours: number }> = {
  "24h": { label: "24 Hours", hours: 24 },
  "7d": { label: "7 Days", hours: 24 * 7 },
};

const STAGE_LABELS: Record<string, string> = {
  signal_age_ms: "Signal Age",
  token_lookup_ms: "Token Lookup",
  depth_check_ms: "Depth Check",
  preflight_ms: "Preflight",
  yes_order_ms: "YES Order",
  no_order_ms: "NO Order",
  inter_leg_gap_ms: "Inter-Leg Gap",
  request_to_fill_ms: "Request to Fill",
  request_to_open_ms: "Request to Open",
  total_time_ms: "Total Attempt",
};

function formatMs(value?: number | null) {
  if (value == null || Number.isNaN(value)) return "—";
  return `${value.toFixed(value >= 100 ? 0 : 1)} ms`;
}

function formatRate(value?: number | null) {
  if (value == null || Number.isNaN(value)) return "—";
  return `${(value * 100).toFixed(1)}%`;
}

function AttemptOutcomeBadge({
  outcome,
  oneLegged,
}: {
  outcome: string;
  oneLegged: boolean;
}) {
  const palette =
    outcome === "opened"
      ? "bg-green-500/10 text-green-600 border-green-500/20"
      : outcome === "failed"
        ? "bg-red-500/10 text-red-600 border-red-500/20"
        : "bg-amber-500/10 text-amber-600 border-amber-500/20";

  return (
    <div className="flex items-center gap-2">
      <Badge variant="outline" className={cn("text-xs", palette)}>
        {outcome}
      </Badge>
      {oneLegged ? (
        <Badge variant="outline" className="text-xs border-red-500/20 bg-red-500/10 text-red-600">
          one-legged
        </Badge>
      ) : null}
    </div>
  );
}

export default function ArbTelemetryPage() {
  const [windowKey, setWindowKey] = useState<WindowKey>("24h");

  const params = useMemo(() => {
    const hours = WINDOWS[windowKey].hours;
    return {
      from: new Date(Date.now() - hours * 60 * 60 * 1000).toISOString(),
      limit: 50,
    };
  }, [windowKey]);

  const { data, isLoading } = useArbExecutionTelemetryQuery(params);

  const sortedLatency = useMemo(() => {
    if (!data) return [];

    return [...data.latency_breakdown].sort((left, right) => {
      const leftWeight = left.stage === "total_time_ms" ? -1 : 0;
      const rightWeight = right.stage === "total_time_ms" ? -1 : 0;
      return leftWeight - rightWeight || left.stage.localeCompare(right.stage);
    });
  }, [data]);

  return (
    <div className="space-y-5 sm:space-y-6">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex items-center gap-3">
          <Gauge className="h-6 w-6 text-muted-foreground" />
          <div>
            <h1 className="text-2xl font-bold">Arb Telemetry</h1>
            <p className="text-sm text-muted-foreground">
              Measure where arb attempts win, stall, skip, or fail
            </p>
          </div>
        </div>
        <LiveIndicator />
      </div>

      <PageIntro
        title="Why this page matters"
        description="Arbitrage win rate is mostly an execution attribution problem. This page isolates the latency and failure points between signal arrival and position open."
        bullets={[
          "Use the top-line cards to watch conversion, one-legged failures, and latency drift.",
          "The stage table shows where the executor spends time before and during entry.",
          "The recent attempts log is the raw material for a later self-improving execution model."
        ]}
      />

      <Tabs value={windowKey} onValueChange={(value) => setWindowKey(value as WindowKey)}>
        <div className="overflow-x-auto">
          <TabsList className="w-max">
            {(Object.keys(WINDOWS) as WindowKey[]).map((key) => (
              <TabsTrigger key={key} value={key}>
                {WINDOWS[key].label}
              </TabsTrigger>
            ))}
          </TabsList>
        </div>
      </Tabs>

      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
        <MetricCard
          title="Entry Requests"
          value={data ? String(data.summary.entry_requests) : "—"}
          trend="neutral"
        />
        <MetricCard
          title="Opened"
          value={data ? String(data.summary.positions_opened) : "—"}
          changeLabel={data ? formatRate(data.summary.success_rate) : undefined}
          trend={(data?.summary.success_rate ?? 0) >= 0.8 ? "up" : "neutral"}
        />
        <MetricCard
          title="Failures"
          value={data ? String(data.summary.position_failures) : "—"}
          changeLabel={data ? formatRate(data.summary.failure_rate) : undefined}
          trend={(data?.summary.position_failures ?? 0) === 0 ? "up" : "down"}
        />
        <MetricCard
          title="Skipped"
          value={data ? String(data.summary.signal_skipped) : "—"}
          changeLabel={data ? formatRate(data.summary.skip_rate) : undefined}
          trend="neutral"
        />
        <MetricCard
          title="One-Legged"
          value={data ? String(data.summary.one_legged_failures) : "—"}
          trend={(data?.summary.one_legged_failures ?? 0) === 0 ? "up" : "down"}
        />
        <MetricCard
          title="Median Total"
          value={data ? formatMs(data.summary.median_total_time_ms) : "—"}
          changeLabel={data ? `P90 ${formatMs(data.summary.p90_total_time_ms)}` : undefined}
          trend="neutral"
        />
        <MetricCard
          title="Median YES Leg"
          value={data ? formatMs(data.summary.median_yes_order_ms) : "—"}
          trend="neutral"
        />
        <MetricCard
          title="Median NO Leg"
          value={data ? formatMs(data.summary.median_no_order_ms) : "—"}
          changeLabel={
            data?.summary.avg_execution_slippage_bps != null
              ? `${data.summary.avg_execution_slippage_bps.toFixed(1)} bps slip`
              : undefined
          }
          trend="neutral"
        />
      </div>

      <div className="grid gap-4 xl:grid-cols-[1.4fr_0.9fr]">
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <TimerReset className="h-4 w-4 text-muted-foreground" />
              <span>Latency Breakdown</span>
              <InfoTooltip content="These samples are read from terminal arb lifecycle events: opened, failed, and skipped attempts." />
            </CardTitle>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <p className="text-sm text-muted-foreground">Loading latency telemetry…</p>
            ) : !data || sortedLatency.length === 0 ? (
              <p className="text-sm text-muted-foreground">No arb telemetry available for this window.</p>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full min-w-[760px]">
                  <thead>
                    <tr className="border-b border-border/60 text-left text-xs uppercase tracking-wide text-muted-foreground">
                      <th className="px-3 py-2 font-medium">Stage</th>
                      <th className="px-3 py-2 font-medium">Samples</th>
                      <th className="px-3 py-2 font-medium">Avg</th>
                      <th className="px-3 py-2 font-medium">P50</th>
                      <th className="px-3 py-2 font-medium">P90</th>
                      <th className="px-3 py-2 font-medium">Max</th>
                    </tr>
                  </thead>
                  <tbody>
                    {sortedLatency.map((row) => (
                      <tr key={row.stage} className="border-b border-border/60 last:border-0">
                        <td className="px-3 py-3 text-sm font-medium">
                          {STAGE_LABELS[row.stage] ?? row.stage}
                        </td>
                        <td className="px-3 py-3 text-sm tabular-nums">{row.sample_size}</td>
                        <td className="px-3 py-3 text-sm tabular-nums">{formatMs(row.avg_ms)}</td>
                        <td className="px-3 py-3 text-sm tabular-nums">{formatMs(row.p50_ms)}</td>
                        <td className="px-3 py-3 text-sm tabular-nums">{formatMs(row.p90_ms)}</td>
                        <td className="px-3 py-3 text-sm tabular-nums">{formatMs(row.max_ms)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <AlertTriangle className="h-4 w-4 text-muted-foreground" />
              <span>Outcome Breakdown</span>
            </CardTitle>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <p className="text-sm text-muted-foreground">Loading breakdown…</p>
            ) : !data || data.outcome_breakdown.length === 0 ? (
              <p className="text-sm text-muted-foreground">No outcome data available.</p>
            ) : (
              <div className="space-y-2">
                {data.outcome_breakdown.slice(0, 10).map((item) => (
                  <div
                    key={`${item.outcome}-${item.reason}`}
                    className="flex items-center justify-between rounded-lg border border-border/60 px-3 py-2"
                  >
                    <div className="min-w-0">
                      <div className="text-sm font-medium">{item.reason}</div>
                      <div className="text-xs text-muted-foreground">{item.outcome}</div>
                    </div>
                    <div className="text-sm tabular-nums text-muted-foreground">{item.count}</div>
                  </div>
                ))}
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader className="pb-3">
          <CardTitle className="flex items-center gap-2 text-base">
            <Activity className="h-4 w-4 text-muted-foreground" />
            <span>Recent Attempts</span>
            <InfoTooltip content="These are terminal arb attempt records. They are the dataset we need before we can train or tune execution policy safely." />
          </CardTitle>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <p className="text-sm text-muted-foreground">Loading recent attempts…</p>
          ) : !data || data.recent_attempts.length === 0 ? (
            <p className="text-sm text-muted-foreground">No attempts found for this window.</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full min-w-[1280px]">
                <thead>
                  <tr className="border-b border-border/60 text-left text-xs uppercase tracking-wide text-muted-foreground">
                    <th className="px-3 py-2 font-medium">Time</th>
                    <th className="px-3 py-2 font-medium">Market</th>
                    <th className="px-3 py-2 font-medium">Outcome</th>
                    <th className="px-3 py-2 font-medium">Reason</th>
                    <th className="px-3 py-2 font-medium">Source</th>
                    <th className="px-3 py-2 font-medium">Signal Age</th>
                    <th className="px-3 py-2 font-medium">Preflight</th>
                    <th className="px-3 py-2 font-medium">YES</th>
                    <th className="px-3 py-2 font-medium">NO</th>
                    <th className="px-3 py-2 font-medium">Total</th>
                    <th className="px-3 py-2 font-medium">Expected</th>
                    <th className="px-3 py-2 font-medium">Observed</th>
                    <th className="px-3 py-2 font-medium">Slip</th>
                  </tr>
                </thead>
                <tbody>
                  {data.recent_attempts.map((attempt) => (
                    <tr
                      key={`${attempt.occurred_at}-${attempt.market_id}-${attempt.position_id ?? attempt.outcome}`}
                      className="border-b border-border/60 last:border-0"
                    >
                      <td className="px-3 py-3 text-xs text-muted-foreground">
                        {new Date(attempt.occurred_at).toLocaleString()}
                      </td>
                      <td className="px-3 py-3 text-xs">
                        <div className="max-w-[260px] truncate" title={attempt.market_id}>
                          {attempt.market_id}
                        </div>
                      </td>
                      <td className="px-3 py-3">
                        <AttemptOutcomeBadge
                          outcome={attempt.outcome}
                          oneLegged={attempt.one_legged}
                        />
                      </td>
                      <td className="px-3 py-3 text-xs text-muted-foreground">
                        {attempt.reason ?? "—"}
                      </td>
                      <td className="px-3 py-3 text-xs text-muted-foreground">
                        {attempt.token_source ?? "—"}
                      </td>
                      <td className="px-3 py-3 text-xs tabular-nums">{formatMs(attempt.signal_age_ms)}</td>
                      <td className="px-3 py-3 text-xs tabular-nums">{formatMs(attempt.preflight_ms)}</td>
                      <td className="px-3 py-3 text-xs tabular-nums">{formatMs(attempt.yes_order_ms)}</td>
                      <td className="px-3 py-3 text-xs tabular-nums">{formatMs(attempt.no_order_ms)}</td>
                      <td className="px-3 py-3 text-xs tabular-nums">{formatMs(attempt.total_time_ms)}</td>
                      <td className="px-3 py-3 text-xs tabular-nums">
                        {attempt.expected_edge != null ? `$${attempt.expected_edge.toFixed(2)}` : "—"}
                      </td>
                      <td
                        className={cn(
                          "px-3 py-3 text-xs tabular-nums",
                          (attempt.observed_edge ?? 0) >= 0 ? "text-profit" : "text-loss",
                        )}
                      >
                        {attempt.observed_edge != null ? `$${attempt.observed_edge.toFixed(2)}` : "—"}
                      </td>
                      <td className="px-3 py-3 text-xs tabular-nums">
                        {attempt.execution_slippage_bps != null
                          ? `${attempt.execution_slippage_bps.toFixed(1)} bps`
                          : "—"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
