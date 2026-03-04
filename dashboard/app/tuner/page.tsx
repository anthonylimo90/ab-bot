"use client";

import { useState } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { MetricCard } from "@/components/shared/MetricCard";
import { LiveIndicator } from "@/components/shared/LiveIndicator";
import { MarketRegimeBadge } from "@/components/shared/MarketRegimeBadge";
import { ErrorBoundary } from "@/components/shared/ErrorBoundary";
import {
  useDynamicTunerQuery,
  useUpdateOpportunitySelectionMutation,
  useUpdateArbExecutorMutation,
} from "@/hooks/queries/useRiskQuery";
import { useWorkspaceStore } from "@/stores/workspace-store";
import {
  formatCurrency,
  formatDynamicKey,
  formatDynamicConfigValue,
  formatTimeAgo,
  cn,
} from "@/lib/utils";
import { SlidersHorizontal } from "lucide-react";
import type { ScannerMarketInsight } from "@/types/api";

type Aggressiveness = "stable" | "balanced" | "discovery";

export default function TunerPage() {
  const { currentWorkspace } = useWorkspaceStore();
  const workspaceId = currentWorkspace?.id;

  const { data: tuner, isLoading } = useDynamicTunerQuery(workspaceId);
  const oppMutation = useUpdateOpportunitySelectionMutation(workspaceId);
  const arbMutation = useUpdateArbExecutorMutation(workspaceId);

  // Opportunity selection local state
  const [aggLevel, setAggLevel] = useState<Aggressiveness | null>(null);
  const [explSlots, setExplSlots] = useState<string>("");

  // Arb executor local state
  const [posSize, setPosSize] = useState<string>("");
  const [minProfit, setMinProfit] = useState<string>("");
  const [minDepth, setMinDepth] = useState<string>("");
  const [maxAge, setMaxAge] = useState<string>("");

  const effectiveAgg =
    aggLevel ?? (tuner?.opportunity_selection?.aggressiveness as Aggressiveness | undefined) ?? "balanced";
  const effectiveSlots =
    explSlots || String(tuner?.opportunity_selection?.exploration_slots ?? "");

  const handleSaveOpp = () => {
    const params: { aggressiveness?: Aggressiveness; exploration_slots?: number } = {};
    if (aggLevel) params.aggressiveness = aggLevel;
    if (explSlots) params.exploration_slots = Number(explSlots);
    oppMutation.mutate(params, {
      onSuccess: () => {
        setAggLevel(null);
        setExplSlots("");
      },
    });
  };

  const handleSaveArb = () => {
    const params: {
      position_size?: number;
      min_net_profit?: number;
      min_book_depth?: number;
      max_signal_age_secs?: number;
    } = {};
    if (posSize) params.position_size = Number(posSize);
    if (minProfit) params.min_net_profit = Number(minProfit);
    if (minDepth) params.min_book_depth = Number(minDepth);
    if (maxAge) params.max_signal_age_secs = Number(maxAge);
    arbMutation.mutate(params, {
      onSuccess: () => {
        setPosSize("");
        setMinProfit("");
        setMinDepth("");
        setMaxAge("");
      },
    });
  };

  return (
    <ErrorBoundary>
      <div className="space-y-5 sm:space-y-6">
        {/* Header */}
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center gap-3">
            <SlidersHorizontal className="h-6 w-6 text-muted-foreground" />
            <div>
              <h1 className="text-2xl font-bold">Tuner</h1>
              <p className="text-sm text-muted-foreground">
                Dynamic configuration and scanner insights
              </p>
            </div>
          </div>
          <div className="flex items-center gap-3">
            <MarketRegimeBadge />
            <LiveIndicator />
          </div>
        </div>

        {isLoading && (
          <div className="flex items-center justify-center py-12">
            <p className="text-muted-foreground">Loading tuner status...</p>
          </div>
        )}

        {tuner && (
          <>
            {/* Status strip */}
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
              <MetricCard
                title="Mode"
                value={tuner.apply_changes ? "Apply" : "Shadow"}
                trend="neutral"
              />
              <MetricCard
                title="Regime"
                value={tuner.current_regime}
                trend="neutral"
              />
              <MetricCard
                title="Frozen"
                value={tuner.frozen ? "Yes" : "No"}
                trend={tuner.frozen ? "down" : "neutral"}
                changeLabel={tuner.freeze_reason ?? undefined}
              />
              <MetricCard
                title="Last Run"
                value={
                  tuner.last_run_at
                    ? formatTimeAgo(tuner.last_run_at)
                    : "Never"
                }
                trend="neutral"
                changeLabel={tuner.last_run_status ?? undefined}
              />
            </div>

            {/* Watchdog Card */}
            <Card>
              <CardHeader className="pb-3">
                <CardTitle className="text-base">Watchdog</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
                  <div className="rounded-lg border p-3">
                    <p className="text-xs text-muted-foreground mb-1">Status</p>
                    <Badge
                      variant={tuner.watchdog_active ? "default" : "secondary"}
                    >
                      {tuner.watchdog_active ? "Active" : "Idle"}
                    </Badge>
                  </div>
                  <div className="rounded-lg border p-3">
                    <p className="text-xs text-muted-foreground mb-1">
                      Fill Rate
                    </p>
                    <p className="text-lg font-bold tabular-nums">
                      {tuner.watchdog_fill_rate != null
                        ? `${(tuner.watchdog_fill_rate * 100).toFixed(1)}%`
                        : "-"}
                    </p>
                  </div>
                  <div className="rounded-lg border p-3">
                    <p className="text-xs text-muted-foreground mb-1">
                      Attempts
                    </p>
                    <p className="text-lg font-bold tabular-nums">
                      {tuner.watchdog_attempts ?? "-"}
                    </p>
                  </div>
                  <div className="rounded-lg border p-3">
                    <p className="text-xs text-muted-foreground mb-1">
                      Top Skip
                    </p>
                    <p className="text-sm font-medium truncate">
                      {tuner.watchdog_top_skip ?? "-"}
                    </p>
                  </div>
                </div>
              </CardContent>
            </Card>

            {/* Opportunity Selection Card */}
            <Card>
              <CardHeader className="pb-3">
                <CardTitle className="text-base">
                  Opportunity Selection
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="flex flex-col sm:flex-row sm:items-end gap-4">
                  <div className="space-y-2">
                    <p className="text-xs text-muted-foreground">
                      Aggressiveness
                    </p>
                    <div className="flex gap-1">
                      {(["stable", "balanced", "discovery"] as const).map(
                        (level) => (
                          <Button
                            key={level}
                            size="sm"
                            variant={
                              effectiveAgg === level ? "default" : "outline"
                            }
                            onClick={() => setAggLevel(level)}
                            className="capitalize"
                          >
                            {level}
                          </Button>
                        ),
                      )}
                    </div>
                  </div>
                  <div className="space-y-2">
                    <p className="text-xs text-muted-foreground">
                      Exploration Slots
                    </p>
                    <Input
                      type="number"
                      value={effectiveSlots}
                      onChange={(e) => setExplSlots(e.target.value)}
                      className="w-24"
                      min={0}
                    />
                  </div>
                  <Button
                    size="sm"
                    onClick={handleSaveOpp}
                    disabled={
                      oppMutation.isPending || (!aggLevel && !explSlots)
                    }
                  >
                    {oppMutation.isPending ? "Saving..." : "Save"}
                  </Button>
                </div>
                {tuner.opportunity_selection && (
                  <p className="text-xs text-muted-foreground">
                    {tuner.opportunity_selection.recommendation}
                  </p>
                )}
              </CardContent>
            </Card>

            {/* Arb Executor Config Card */}
            <Card>
              <CardHeader className="pb-3">
                <CardTitle className="text-base">
                  Arb Executor Config
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
                  <div className="space-y-1">
                    <p className="text-xs text-muted-foreground">
                      Position Size ($)
                    </p>
                    <Input
                      type="number"
                      placeholder={formatDynamicConfigValue(
                        "ARB_POSITION_SIZE",
                        tuner.dynamic_config.find(
                          (c) => c.key === "ARB_POSITION_SIZE",
                        )?.current_value ?? null,
                      )}
                      value={posSize}
                      onChange={(e) => setPosSize(e.target.value)}
                    />
                  </div>
                  <div className="space-y-1">
                    <p className="text-xs text-muted-foreground">
                      Min Net Profit
                    </p>
                    <Input
                      type="number"
                      step="0.0001"
                      placeholder={formatDynamicConfigValue(
                        "ARB_MIN_NET_PROFIT",
                        tuner.dynamic_config.find(
                          (c) => c.key === "ARB_MIN_NET_PROFIT",
                        )?.current_value ?? null,
                      )}
                      value={minProfit}
                      onChange={(e) => setMinProfit(e.target.value)}
                    />
                  </div>
                  <div className="space-y-1">
                    <p className="text-xs text-muted-foreground">
                      Min Book Depth ($)
                    </p>
                    <Input
                      type="number"
                      placeholder={formatDynamicConfigValue(
                        "ARB_MIN_BOOK_DEPTH",
                        tuner.dynamic_config.find(
                          (c) => c.key === "ARB_MIN_BOOK_DEPTH",
                        )?.current_value ?? null,
                      )}
                      value={minDepth}
                      onChange={(e) => setMinDepth(e.target.value)}
                    />
                  </div>
                  <div className="space-y-1">
                    <p className="text-xs text-muted-foreground">
                      Max Signal Age (s)
                    </p>
                    <Input
                      type="number"
                      placeholder={formatDynamicConfigValue(
                        "ARB_MAX_SIGNAL_AGE_SECS",
                        tuner.dynamic_config.find(
                          (c) => c.key === "ARB_MAX_SIGNAL_AGE_SECS",
                        )?.current_value ?? null,
                      )}
                      value={maxAge}
                      onChange={(e) => setMaxAge(e.target.value)}
                    />
                  </div>
                </div>
                <Button
                  size="sm"
                  onClick={handleSaveArb}
                  disabled={
                    arbMutation.isPending ||
                    (!posSize && !minProfit && !minDepth && !maxAge)
                  }
                >
                  {arbMutation.isPending ? "Saving..." : "Save"}
                </Button>
              </CardContent>
            </Card>

            {/* Dynamic Config Table */}
            <Card>
              <CardHeader className="pb-3">
                <CardTitle className="text-base">
                  Dynamic Config Parameters
                </CardTitle>
              </CardHeader>
              <CardContent className="p-0">
                <div className="overflow-x-auto">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b bg-muted/50">
                        <th className="px-4 py-2 text-left font-medium text-muted-foreground">
                          Key
                        </th>
                        <th className="px-4 py-2 text-right font-medium text-muted-foreground">
                          Current
                        </th>
                        <th className="px-4 py-2 text-right font-medium text-muted-foreground">
                          Min
                        </th>
                        <th className="px-4 py-2 text-right font-medium text-muted-foreground">
                          Max
                        </th>
                        <th className="px-4 py-2 text-right font-medium text-muted-foreground">
                          Step
                        </th>
                        <th className="px-4 py-2 text-center font-medium text-muted-foreground">
                          Status
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {tuner.dynamic_config.map((item) => (
                        <tr
                          key={item.key}
                          className="border-b hover:bg-muted/30 transition-colors"
                        >
                          <td className="px-4 py-2 font-medium">
                            {formatDynamicKey(item.key)}
                          </td>
                          <td className="px-4 py-2 text-right tabular-nums">
                            {formatDynamicConfigValue(item.key, item.current_value)}
                          </td>
                          <td className="px-4 py-2 text-right tabular-nums text-muted-foreground">
                            {formatDynamicConfigValue(item.key, item.min_value)}
                          </td>
                          <td className="px-4 py-2 text-right tabular-nums text-muted-foreground">
                            {formatDynamicConfigValue(item.key, item.max_value)}
                          </td>
                          <td className="px-4 py-2 text-right tabular-nums text-muted-foreground">
                            {(item.max_step_pct * 100).toFixed(0)}%
                          </td>
                          <td className="px-4 py-2 text-center">
                            {item.pending_eval ? (
                              <Badge
                                variant="outline"
                                className="bg-yellow-500/10 text-yellow-600 border-yellow-500/20 text-xs"
                              >
                                Pending
                              </Badge>
                            ) : (
                              <Badge variant="secondary" className="text-xs">
                                Stable
                              </Badge>
                            )}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </CardContent>
            </Card>

            {/* Scanner Market Insights */}
            {tuner.scanner_status?.selected_markets?.length > 0 && (
              <Card>
                <CardHeader className="pb-3">
                  <div className="flex items-center justify-between">
                    <CardTitle className="text-base">
                      Scanner Market Insights
                    </CardTitle>
                    <Badge variant="secondary">
                      {tuner.scanner_status.selected_markets.length} markets
                    </Badge>
                  </div>
                </CardHeader>
                <CardContent className="p-0">
                  <div className="overflow-x-auto">
                    <table className="w-full text-sm">
                      <thead>
                        <tr className="border-b bg-muted/50">
                          <th className="px-3 py-2 text-left font-medium text-muted-foreground">
                            Market ID
                          </th>
                          <th className="px-3 py-2 text-center font-medium text-muted-foreground">
                            Tier
                          </th>
                          <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                            Total
                          </th>
                          <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                            Base
                          </th>
                          <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                            Opp
                          </th>
                          <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                            Hit Rate
                          </th>
                          <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                            Fresh
                          </th>
                          <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                            Sticky
                          </th>
                          <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                            Novel
                          </th>
                          <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                            Rot
                          </th>
                          <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                            Upside
                          </th>
                        </tr>
                      </thead>
                      <tbody>
                        {[...tuner.scanner_status.selected_markets]
                          .sort(
                            (a: ScannerMarketInsight, b: ScannerMarketInsight) =>
                              b.total_score - a.total_score,
                          )
                          .map((m: ScannerMarketInsight) => (
                            <tr
                              key={m.market_id}
                              className="border-b hover:bg-muted/30 transition-colors"
                            >
                              <td className="px-3 py-2">
                                <span
                                  className="font-mono text-xs truncate block max-w-[120px]"
                                  title={m.market_id}
                                >
                                  {m.market_id.slice(0, 12)}...
                                </span>
                              </td>
                              <td className="px-3 py-2 text-center">
                                <Badge
                                  variant="outline"
                                  className={cn(
                                    "text-xs",
                                    m.tier === "core"
                                      ? "bg-green-500/10 text-green-600 border-green-500/20"
                                      : "bg-blue-500/10 text-blue-600 border-blue-500/20",
                                  )}
                                >
                                  {m.tier}
                                </Badge>
                              </td>
                              <td className="px-3 py-2 text-right tabular-nums font-medium">
                                {m.total_score.toFixed(2)}
                              </td>
                              <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
                                {m.baseline_score.toFixed(2)}
                              </td>
                              <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
                                {m.opportunity_score.toFixed(2)}
                              </td>
                              <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
                                {m.hit_rate_score.toFixed(2)}
                              </td>
                              <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
                                {m.freshness_score.toFixed(2)}
                              </td>
                              <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
                                {m.sticky_score.toFixed(2)}
                              </td>
                              <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
                                {m.novelty_score?.toFixed(2) ?? "-"}
                              </td>
                              <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
                                {m.rotation_score?.toFixed(2) ?? "-"}
                              </td>
                              <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
                                {m.upside_score?.toFixed(2) ?? "-"}
                              </td>
                            </tr>
                          ))}
                      </tbody>
                    </table>
                  </div>
                </CardContent>
              </Card>
            )}
          </>
        )}
      </div>
    </ErrorBoundary>
  );
}
