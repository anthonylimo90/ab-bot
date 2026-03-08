"use client";

import { useEffect, useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { MetricCard } from "@/components/shared/MetricCard";
import { PageIntro } from "@/components/shared/PageIntro";
import { LiveIndicator } from "@/components/shared/LiveIndicator";
import { InfoTooltip } from "@/components/shared/InfoTooltip";
import { useTradeFlowJourneysQuery, useTradeFlowSummaryQuery } from "@/hooks/queries/useTradeFlowQuery";
import { useWebSocket } from "@/hooks/useWebSocket";
import { formatCurrency, cn } from "@/lib/utils";
import type { TradeFlowEvent, TradeJourney, WebSocketMessage } from "@/types/api";
import { GitBranch, ArrowRightLeft, Clock3, Activity } from "lucide-react";

type StrategyFilter =
  | "all"
  | "arb"
  | "flow"
  | "mean_reversion"
  | "cross_market"
  | "resolution_proximity";

const STRATEGY_LABELS: Record<StrategyFilter, string> = {
  all: "All",
  arb: "Arb",
  flow: "Flow",
  mean_reversion: "Mean Rev",
  cross_market: "Cross Mkt",
  resolution_proximity: "Resolution",
};

function StageBadge({ stage }: { stage: string }) {
  const palette =
    stage === "closed"
      ? "bg-muted text-muted-foreground border-muted-foreground/20"
      : stage === "open"
        ? "bg-green-500/10 text-green-600 border-green-500/20"
        : stage === "exit_ready"
          ? "bg-blue-500/10 text-blue-600 border-blue-500/20"
          : stage.includes("failed")
            ? "bg-red-500/10 text-red-600 border-red-500/20"
            : stage === "skipped" || stage === "expired"
              ? "bg-amber-500/10 text-amber-600 border-amber-500/20"
              : "bg-slate-500/10 text-slate-600 border-slate-500/20";
  return (
    <Badge variant="outline" className={cn("text-xs", palette)}>
      {stage.replace(/_/g, " ")}
    </Badge>
  );
}

function formatTimestamp(value?: string | null) {
  if (!value) return "—";
  return new Date(value).toLocaleString();
}

function JourneyRow({ journey }: { journey: TradeJourney }) {
  return (
    <tr className="border-b border-border/60 last:border-0">
      <td className="px-3 py-3 text-sm font-medium">{journey.strategy}</td>
      <td className="px-3 py-3 text-xs text-muted-foreground">
        <div className="truncate max-w-[220px]" title={journey.market_id}>
          {journey.market_id}
        </div>
      </td>
      <td className="px-3 py-3">
        <StageBadge stage={journey.lifecycle_stage} />
      </td>
      <td className="px-3 py-3 text-xs text-muted-foreground">
        {journey.supports_signal_history ? "full" : "partial"}
      </td>
      <td className="px-3 py-3 text-xs">{journey.direction ?? "—"}</td>
      <td className="px-3 py-3 text-xs tabular-nums">
        {journey.confidence != null ? `${(journey.confidence * 100).toFixed(0)}%` : "—"}
      </td>
      <td className="px-3 py-3 text-xs">{formatTimestamp(journey.signal_generated_at)}</td>
      <td className="px-3 py-3 text-xs">{formatTimestamp(journey.opened_at)}</td>
      <td className="px-3 py-3 text-xs">{formatTimestamp(journey.closed_at)}</td>
      <td
        className={cn(
          "px-3 py-3 text-xs tabular-nums",
          (journey.realized_pnl ?? 0) >= 0 ? "text-profit" : "text-loss",
        )}
      >
        {journey.realized_pnl != null ? formatCurrency(journey.realized_pnl, { showSign: true }) : "—"}
      </td>
    </tr>
  );
}

export default function TradeFlowPage() {
  const [strategy, setStrategy] = useState<StrategyFilter>("all");
  const [liveEvents, setLiveEvents] = useState<TradeFlowEvent[]>([]);
  const queryStrategy = strategy === "all" ? undefined : strategy;

  useEffect(() => {
    setLiveEvents([]);
  }, [strategy]);

  const { data: summary, isLoading: summaryLoading } = useTradeFlowSummaryQuery({
    strategy: queryStrategy,
    limit: 100,
  });
  const { data: journeys = [], isLoading: journeysLoading } = useTradeFlowJourneysQuery({
    strategy: queryStrategy,
    limit: 100,
  });

  const metrics = useMemo(() => {
    if (!summary) {
      return {
        executionRate: 0,
        avgHoldHours: null as number | null,
      };
    }

    const executionRate =
      summary.total_generated_signals > 0
        ? (summary.total_executed_signals / summary.total_generated_signals) * 100
        : 0;

    const holdCandidates = summary.strategies
      .map((item) => item.avg_hold_hours)
      .filter((value): value is number => value != null);

    const avgHoldHours =
      holdCandidates.length > 0
        ? holdCandidates.reduce((sum, value) => sum + value, 0) / holdCandidates.length
        : null;

    return { executionRate, avgHoldHours };
  }, [summary]);

  useWebSocket({
    channel: "trade-flow",
    batchMessages: true,
    batchInterval: 250,
    onMessageBatch: (messages: WebSocketMessage[]) => {
      const updates = messages
        .filter((message): message is { type: "TradeFlow"; data: TradeFlowEvent } => message.type === "TradeFlow")
        .map((message) => message.data)
        .filter((event) => strategy === "all" || event.strategy === strategy);

      if (updates.length === 0) return;

      setLiveEvents((current) => {
        const merged = [...updates.reverse(), ...current];
        const seen = new Set<string>();
        return merged.filter((event) => {
          if (seen.has(event.id)) return false;
          seen.add(event.id);
          return true;
        }).slice(0, 20);
      });
    },
  });

  return (
    <div className="space-y-5 sm:space-y-6">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex items-center gap-3">
          <GitBranch className="h-6 w-6 text-muted-foreground" />
          <div>
            <h1 className="text-2xl font-bold">Trade Flow</h1>
            <p className="text-sm text-muted-foreground">
              See how signals turn into positions, closes, and failures
            </p>
          </div>
        </div>
        <LiveIndicator />
      </div>

      <PageIntro
        title="How to read trade flow"
        description="This page shows the lifecycle of current trading activity. Quant strategies have fuller signal history, while arb history is partial until a canonical event stream is added."
        bullets={[
          "Use the summary cards to see throughput, conversion, and realized outcomes.",
          "The strategy table shows where trades are succeeding, failing, or getting filtered out.",
          "The journey table helps you find where a single trade currently sits in the lifecycle."
        ]}
      />

      <Tabs value={strategy} onValueChange={(value) => setStrategy(value as StrategyFilter)}>
        <div className="overflow-x-auto">
          <TabsList className="w-max">
            {(Object.keys(STRATEGY_LABELS) as StrategyFilter[]).map((key) => (
              <TabsTrigger key={key} value={key}>
                {STRATEGY_LABELS[key]}
              </TabsTrigger>
            ))}
          </TabsList>
        </div>
      </Tabs>

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-5">
        <MetricCard
          title="Signals Generated"
          value={summary ? String(summary.total_generated_signals) : "—"}
          trend="neutral"
        />
        <MetricCard
          title="Signals Executed"
          value={summary ? String(summary.total_executed_signals) : "—"}
          changeLabel={summary ? `${metrics.executionRate.toFixed(1)}% conversion` : undefined}
          trend={metrics.executionRate >= 50 ? "up" : metrics.executionRate > 0 ? "neutral" : "down"}
        />
        <MetricCard
          title="Open Positions"
          value={summary ? String(summary.total_open_positions) : "—"}
          trend="neutral"
        />
        <MetricCard
          title="Closed Positions"
          value={summary ? String(summary.total_closed_positions) : "—"}
          trend="neutral"
        />
        <MetricCard
          title="Realized P&L"
          value={summary ? formatCurrency(summary.total_realized_pnl, { showSign: true }) : "—"}
          changeLabel={metrics.avgHoldHours != null ? `${metrics.avgHoldHours.toFixed(1)}h avg hold` : undefined}
          trend={(summary?.total_realized_pnl ?? 0) >= 0 ? "up" : "down"}
        />
      </div>

      <Card>
        <CardHeader className="pb-3">
          <CardTitle className="flex items-center gap-2 text-base">
            <Activity className="h-4 w-4 text-muted-foreground" />
            <span>Live Lifecycle Events</span>
            <InfoTooltip content="This stream is driven by the canonical trade_events table and websocket feed. It shows new lifecycle transitions as they happen." />
          </CardTitle>
        </CardHeader>
        <CardContent>
          {liveEvents.length === 0 ? (
            <p className="text-sm text-muted-foreground">Waiting for live trade-flow events…</p>
          ) : (
            <div className="space-y-2">
              {liveEvents.map((event) => (
                <div
                  key={event.id}
                  className="flex flex-col gap-1 rounded-lg border border-border/60 px-3 py-2 sm:flex-row sm:items-center sm:justify-between"
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <Badge variant="outline" className="text-xs">
                        {event.strategy}
                      </Badge>
                      <StageBadge stage={event.state_to ?? event.event_type} />
                      <span className="text-xs text-muted-foreground">{event.event_type.replace(/_/g, " ")}</span>
                    </div>
                    <div className="truncate text-xs text-muted-foreground" title={event.market_id}>
                      {event.market_id}
                    </div>
                  </div>
                  <div className="flex items-center gap-3 text-xs text-muted-foreground">
                    <span>{event.execution_mode}</span>
                    <span>{new Date(event.occurred_at).toLocaleTimeString()}</span>
                    <span className={cn(Number(event.realized_pnl ?? 0) >= 0 ? "text-profit" : "text-loss")}>
                      {event.realized_pnl != null
                        ? formatCurrency(Number(event.realized_pnl), { showSign: true })
                        : event.reason ?? "—"}
                    </span>
                  </div>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="pb-3">
          <CardTitle className="flex items-center gap-2 text-base">
            <ArrowRightLeft className="h-4 w-4 text-muted-foreground" />
            <span>Strategy Flow Summary</span>
            <InfoTooltip content="Quant strategies include generated, skipped, and expired signal counts. Arb currently starts later in the lifecycle, so its signal-history coverage is marked partial." />
          </CardTitle>
        </CardHeader>
        <CardContent>
          {summaryLoading ? (
            <p className="text-sm text-muted-foreground">Loading trade-flow summary…</p>
          ) : !summary || summary.strategies.length === 0 ? (
            <p className="text-sm text-muted-foreground">No trade-flow data available for this filter.</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full min-w-[980px]">
                <thead>
                  <tr className="border-b border-border/60 text-left text-xs uppercase tracking-wide text-muted-foreground">
                    <th className="px-3 py-2 font-medium">Strategy</th>
                    <th className="px-3 py-2 font-medium">History</th>
                    <th className="px-3 py-2 font-medium">Generated</th>
                    <th className="px-3 py-2 font-medium">Executed</th>
                    <th className="px-3 py-2 font-medium">Skipped</th>
                    <th className="px-3 py-2 font-medium">Expired</th>
                    <th className="px-3 py-2 font-medium">Open</th>
                    <th className="px-3 py-2 font-medium">Exit Ready</th>
                    <th className="px-3 py-2 font-medium">Closed</th>
                    <th className="px-3 py-2 font-medium">Failures</th>
                    <th className="px-3 py-2 font-medium">Net P&L</th>
                  </tr>
                </thead>
                <tbody>
                  {summary.strategies.map((item) => (
                    <tr key={`${item.source}-${item.strategy}`} className="border-b border-border/60 last:border-0">
                      <td className="px-3 py-3 text-sm font-medium">{item.strategy}</td>
                      <td className="px-3 py-3 text-xs">
                        <Badge variant="outline" className={cn("text-xs", item.supports_signal_history ? "bg-green-500/10 text-green-600 border-green-500/20" : "bg-amber-500/10 text-amber-600 border-amber-500/20")}>
                          {item.supports_signal_history ? "full" : "partial"}
                        </Badge>
                      </td>
                      <td className="px-3 py-3 text-sm tabular-nums">{item.generated_signals}</td>
                      <td className="px-3 py-3 text-sm tabular-nums">{item.executed_signals}</td>
                      <td className="px-3 py-3 text-sm tabular-nums">{item.skipped_signals}</td>
                      <td className="px-3 py-3 text-sm tabular-nums">{item.expired_signals}</td>
                      <td className="px-3 py-3 text-sm tabular-nums">{item.open_positions}</td>
                      <td className="px-3 py-3 text-sm tabular-nums">{item.exit_ready_positions}</td>
                      <td className="px-3 py-3 text-sm tabular-nums">{item.closed_positions}</td>
                      <td className="px-3 py-3 text-sm tabular-nums">{item.entry_failed_positions + item.exit_failed_positions}</td>
                      <td className={cn("px-3 py-3 text-sm tabular-nums", item.net_pnl >= 0 ? "text-profit" : "text-loss")}>
                        {formatCurrency(item.net_pnl, { showSign: true })}
                      </td>
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
            <Clock3 className="h-4 w-4 text-muted-foreground" />
            <span>Recent Trade Journeys</span>
            <InfoTooltip content="Each row is the latest lifecycle view for a trade idea or position. Arb rows are marked synthetic because current persisted history starts at execution/position state, not at signal generation." />
          </CardTitle>
        </CardHeader>
        <CardContent>
          {journeysLoading ? (
            <p className="text-sm text-muted-foreground">Loading recent journeys…</p>
          ) : journeys.length === 0 ? (
            <p className="text-sm text-muted-foreground">No journeys available for this filter.</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full min-w-[1220px]">
                <thead>
                  <tr className="border-b border-border/60 text-left text-xs uppercase tracking-wide text-muted-foreground">
                    <th className="px-3 py-2 font-medium">Strategy</th>
                    <th className="px-3 py-2 font-medium">Market</th>
                    <th className="px-3 py-2 font-medium">Stage</th>
                    <th className="px-3 py-2 font-medium">History</th>
                    <th className="px-3 py-2 font-medium">Direction</th>
                    <th className="px-3 py-2 font-medium">Confidence</th>
                    <th className="px-3 py-2 font-medium">Generated</th>
                    <th className="px-3 py-2 font-medium">Opened</th>
                    <th className="px-3 py-2 font-medium">Closed</th>
                    <th className="px-3 py-2 font-medium">P&L</th>
                  </tr>
                </thead>
                <tbody>
                  {journeys.map((journey) => (
                    <JourneyRow
                      key={`${journey.strategy}-${journey.signal_id ?? journey.position_id ?? journey.market_id}-${journey.lifecycle_stage}`}
                      journey={journey}
                    />
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
            <Activity className="h-4 w-4 text-muted-foreground" />
            <span>Implementation Notes</span>
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-2 text-sm text-muted-foreground">
          <p>Quant flow is reconstructed from persisted signals and linked positions.</p>
          <p>Arb flow is reconstructed from positions only, so pre-execution arb filtering history is not yet available.</p>
          <p>Historical arb rows are flagged as partial until a canonical lifecycle event stream is added.</p>
        </CardContent>
      </Card>
    </div>
  );
}
