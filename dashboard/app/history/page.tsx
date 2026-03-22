"use client";

import { useMemo, useState } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Input } from "@/components/ui/input";
import { MetricCard } from "@/components/shared/MetricCard";
import { ErrorBoundary } from "@/components/shared/ErrorBoundary";
import { PageIntro } from "@/components/shared/PageIntro";
import { PositionTableSkeleton } from "@/components/shared/Skeletons";
import { PortfolioChart } from "@/components/charts/PortfolioChart";
import { useDynamicConfigHistoryQuery } from "@/hooks/queries/useHistoryQuery";
import {
  useAccountHistoryQuery,
  useCreateCashFlowMutation,
} from "@/hooks/queries/useAccountQuery";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { useToastStore } from "@/stores/toast-store";
import {
  formatCurrency,
  formatTimeAgo,
  formatDynamicKey,
  formatDynamicConfigValue,
} from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { History, LineChart, Landmark, TrendingUp, Wallet, SlidersHorizontal } from "lucide-react";

type HistoryTab = "equity" | "trades" | "cash_flows" | "dynamic";

function cashFlowLabel(type: string) {
  return type.replace(/_/g, " ").replace(/\b\w/g, (char) => char.toUpperCase());
}

export default function HistoryPage() {
  const { currentWorkspace } = useWorkspaceStore();
  const workspaceId = currentWorkspace?.id;
  const toast = useToastStore();
  const canManageCashFlows =
    currentWorkspace?.my_role === "owner" || currentWorkspace?.my_role === "admin";

  const [historyTab, setHistoryTab] = useState<HistoryTab>("equity");
  const [cashFlowType, setCashFlowType] = useState("deposit");
  const [cashFlowAmount, setCashFlowAmount] = useState("");
  const [cashFlowNote, setCashFlowNote] = useState("");
  const [cashFlowOccurredAt, setCashFlowOccurredAt] = useState("");

  const {
    data: accountHistory,
    isLoading: accountLoading,
    error: accountError,
  } = useAccountHistoryQuery(workspaceId, { hours: 24, limit: 100 });

  const {
    data: dynamicHistory = [],
    isLoading: dynamicLoading,
    error: dynamicError,
  } = useDynamicConfigHistoryQuery({
    workspaceId,
    limit: 50,
    offset: 0,
  });

  const createCashFlowMutation = useCreateCashFlowMutation(workspaceId);

  const summary = accountHistory?.summary;
  const equityCurve = useMemo(
    () =>
      (accountHistory?.equity_curve ?? []).map((point) => ({
        time: new Date(point.snapshot_time).toISOString(),
        value: point.total_equity,
      })),
    [accountHistory],
  );

  const handleCreateCashFlow = async () => {
    const amount = Number(cashFlowAmount);
    if (!Number.isFinite(amount) || amount === 0) {
      toast.error("Invalid amount", "Enter a non-zero cash flow amount");
      return;
    }

    try {
      await createCashFlowMutation.mutateAsync({
        event_type: cashFlowType,
        amount,
        note: cashFlowNote || undefined,
        occurred_at: cashFlowOccurredAt
          ? new Date(cashFlowOccurredAt).toISOString()
          : undefined,
      });
      setCashFlowAmount("");
      setCashFlowNote("");
      setCashFlowOccurredAt("");
      toast.success("Cash flow recorded", "The account ledger has been updated");
    } catch (error) {
      toast.error(
        "Failed to record cash flow",
        error instanceof Error ? error.message : "Unknown error",
      );
    }
  };

  return (
    <ErrorBoundary>
      <div className="space-y-5 sm:space-y-6">
        <div>
          <h1 className="flex items-center gap-2 text-2xl font-bold tracking-tight sm:text-3xl">
            <History className="h-8 w-8" />
            History
          </h1>
          <p className="text-muted-foreground">
            Canonical account snapshots, trade lifecycle events, and cash flows
          </p>
        </div>

        <PageIntro
          title="What this history uses"
          description="This page now reads from the account ledger and reconciled wallet inventory instead of the old activity feed. Equity comes from periodic snapshots, trade activity comes from trade events, and deposits or withdrawals are tracked separately as cash flows."
          bullets={[
            "Account Equity = wallet cash + reconciled inventory value.",
            "Trade history is sourced from canonical trade events, not execution-report messages.",
            "Cash flows are recorded separately so top-ups and withdrawals do not look like trading P&L.",
          ]}
        />

        {accountLoading ? (
          <PositionTableSkeleton rows={6} />
        ) : accountError || !summary ? (
          <Card>
            <CardContent className="p-12 text-center">
              <p className="text-destructive">
                Failed to load account history.
              </p>
            </CardContent>
          </Card>
        ) : (
          <>
            <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-5">
              <MetricCard
                title="Account Equity"
                value={formatCurrency(summary.total_equity)}
                trend={summary.total_equity >= 0 ? "up" : "neutral"}
              />
              <MetricCard
                title="Cash Balance"
                value={formatCurrency(summary.cash_balance)}
                trend="neutral"
              />
              <MetricCard
                title="Open Exposure"
                value={formatCurrency(summary.position_value)}
                trend="neutral"
                changeLabel={`${summary.open_positions} active item${summary.open_positions === 1 ? '' : 's'}`}
              />
              <MetricCard
                title="Realized P&L 24h"
                value={formatCurrency(summary.realized_pnl_24h, { showSign: true })}
                trend={summary.realized_pnl_24h >= 0 ? "up" : "down"}
                changeLabel={`Cash flows 24h ${formatCurrency(summary.net_cash_flows_24h, { showSign: true })}`}
              />
              <MetricCard
                title="Orphan Inventory"
                value={formatCurrency(summary.orphan_marked_value)}
                trend={summary.orphan_positions > 0 ? "down" : "neutral"}
                changeLabel={`${summary.orphan_positions} orphan item${summary.orphan_positions === 1 ? '' : 's'}`}
              />
            </div>

            <Tabs
              value={historyTab}
              onValueChange={(value) => setHistoryTab(value as HistoryTab)}
            >
              <div className="overflow-x-auto pb-1">
                <TabsList className="grid w-full min-w-[28rem] grid-cols-4">
                  <TabsTrigger value="equity">Equity</TabsTrigger>
                  <TabsTrigger value="trades">Trade Events</TabsTrigger>
                  <TabsTrigger value="cash_flows">Cash Flows</TabsTrigger>
                  <TabsTrigger value="dynamic">Dynamic Config</TabsTrigger>
                </TabsList>
              </div>

              <TabsContent value="equity" className="mt-4 space-y-4">
                <Card>
                  <CardHeader>
                    <CardTitle>Equity Curve</CardTitle>
                  </CardHeader>
                  <CardContent className="space-y-4">
                    {equityCurve.length > 1 ? (
                      <PortfolioChart data={equityCurve} height={360} />
                    ) : (
                      <div className="flex h-[360px] items-center justify-center text-sm text-muted-foreground">
                        Snapshots are still building. The current ledger started at{" "}
                        {accountHistory.snapshot_started_at
                          ? formatTimeAgo(accountHistory.snapshot_started_at)
                          : "the latest refresh"}
                        .
                      </div>
                    )}

                    <div className="grid gap-3 md:grid-cols-4">
                      <div className="rounded-lg border p-4">
                        <p className="text-xs text-muted-foreground">Last Snapshot</p>
                        <p className="mt-1 text-sm font-medium">
                          {formatTimeAgo(summary.snapshot_time)}
                        </p>
                      </div>
                      <div className="rounded-lg border p-4">
                        <p className="text-xs text-muted-foreground">Unpriced Holdings</p>
                        <p className="mt-1 text-sm font-medium">
                          {summary.unpriced_open_positions}
                        </p>
                        <p className="mt-1 text-xs text-muted-foreground">
                          Cost basis {formatCurrency(summary.unpriced_position_cost_basis)}
                        </p>
                      </div>
                      <div className="rounded-lg border p-4">
                        <p className="text-xs text-muted-foreground">Unrealized P&amp;L</p>
                        <p className="mt-1 text-sm font-medium">
                          {formatCurrency(summary.unrealized_pnl, { showSign: true })}
                        </p>
                      </div>
                      <div className="rounded-lg border p-4">
                        <p className="text-xs text-muted-foreground">Inventory Discovery</p>
                        <p className="mt-1 text-sm font-medium">
                          {summary.inventory_backfill_in_progress
                            ? "Backfill in progress"
                            : summary.inventory_backfill_completed_at
                              ? "Backfill complete"
                              : "Awaiting sync"}
                        </p>
                        <p className="mt-1 text-xs text-muted-foreground">
                          {summary.inventory_last_synced_at
                            ? `Inventory sync ${formatTimeAgo(summary.inventory_last_synced_at)}`
                            : "No inventory sync recorded yet"}
                        </p>
                      </div>
                    </div>
                  </CardContent>
                </Card>
              </TabsContent>

              <TabsContent value="trades" className="mt-4">
                <Card>
                  <CardHeader>
                    <CardTitle className="flex items-center gap-2">
                      <LineChart className="h-5 w-5" />
                      Recent Trade Events
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <div className="overflow-x-auto">
                      <table className="w-full text-left">
                        <thead>
                          <tr className="border-b text-xs text-muted-foreground">
                            <th className="px-3 py-2">Time</th>
                            <th className="px-3 py-2">Strategy</th>
                            <th className="px-3 py-2">Event</th>
                            <th className="px-3 py-2">Market</th>
                            <th className="px-3 py-2">Reason</th>
                            <th className="px-3 py-2 text-right">P&amp;L</th>
                          </tr>
                        </thead>
                        <tbody>
                          {accountHistory.recent_trade_events.map((event) => (
                            <tr key={event.id} className="border-b last:border-b-0">
                              <td className="px-3 py-3 text-sm text-muted-foreground">
                                {formatTimeAgo(event.occurred_at)}
                              </td>
                              <td className="px-3 py-3 text-sm">
                                <div className="font-medium">{event.strategy}</div>
                                <div className="text-xs text-muted-foreground">
                                  {event.source} · {event.execution_mode}
                                </div>
                              </td>
                              <td className="px-3 py-3 text-sm">
                                <Badge variant="outline">{event.event_type}</Badge>
                              </td>
                              <td className="px-3 py-3 text-sm font-mono text-xs">
                                {event.market_id.slice(0, 16)}...
                              </td>
                              <td className="px-3 py-3 text-sm text-muted-foreground">
                                {event.reason ?? "—"}
                              </td>
                              <td className="px-3 py-3 text-right text-sm tabular-nums">
                                {event.realized_pnl != null
                                  ? formatCurrency(event.realized_pnl, { showSign: true })
                                  : event.unrealized_pnl != null
                                    ? formatCurrency(event.unrealized_pnl, { showSign: true })
                                    : "—"}
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  </CardContent>
                </Card>
              </TabsContent>

              <TabsContent value="cash_flows" className="mt-4 space-y-4">
                {canManageCashFlows && (
                  <Card>
                    <CardHeader>
                      <CardTitle className="flex items-center gap-2">
                        <Wallet className="h-5 w-5" />
                        Record Cash Flow
                      </CardTitle>
                    </CardHeader>
                    <CardContent className="grid gap-3 md:grid-cols-2">
                      <label className="space-y-2 text-sm">
                        <span className="font-medium">Type</span>
                        <select
                          value={cashFlowType}
                          onChange={(e) => setCashFlowType(e.target.value)}
                          className="w-full rounded border bg-background px-3 py-2"
                        >
                          <option value="deposit">Deposit</option>
                          <option value="withdrawal">Withdrawal</option>
                          <option value="transfer">Transfer</option>
                          <option value="fee">Fee</option>
                          <option value="adjustment">Adjustment</option>
                        </select>
                      </label>
                      <label className="space-y-2 text-sm">
                        <span className="font-medium">Amount</span>
                        <Input
                          value={cashFlowAmount}
                          onChange={(e) => setCashFlowAmount(e.target.value)}
                          placeholder="100 or -25"
                        />
                      </label>
                      <label className="space-y-2 text-sm">
                        <span className="font-medium">Occurred At</span>
                        <Input
                          type="datetime-local"
                          value={cashFlowOccurredAt}
                          onChange={(e) => setCashFlowOccurredAt(e.target.value)}
                        />
                      </label>
                      <label className="space-y-2 text-sm">
                        <span className="font-medium">Note</span>
                        <Input
                          value={cashFlowNote}
                          onChange={(e) => setCashFlowNote(e.target.value)}
                          placeholder="Top-up, manual withdrawal, fee adjustment..."
                        />
                      </label>
                      <div className="md:col-span-2">
                        <Button
                          onClick={handleCreateCashFlow}
                          disabled={createCashFlowMutation.isPending}
                        >
                          {createCashFlowMutation.isPending
                            ? "Recording..."
                            : "Record Cash Flow"}
                        </Button>
                      </div>
                    </CardContent>
                  </Card>
                )}

                <Card>
                  <CardHeader>
                    <CardTitle className="flex items-center gap-2">
                      <Landmark className="h-5 w-5" />
                      Cash Flow Ledger
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <div className="overflow-x-auto">
                      <table className="w-full text-left">
                        <thead>
                          <tr className="border-b text-xs text-muted-foreground">
                            <th className="px-3 py-2">Time</th>
                            <th className="px-3 py-2">Type</th>
                            <th className="px-3 py-2">Note</th>
                            <th className="px-3 py-2 text-right">Amount</th>
                          </tr>
                        </thead>
                        <tbody>
                          {accountHistory.cash_flows.map((flow) => (
                            <tr key={flow.id} className="border-b last:border-b-0">
                              <td className="px-3 py-3 text-sm text-muted-foreground">
                                {formatTimeAgo(flow.occurred_at)}
                              </td>
                              <td className="px-3 py-3 text-sm font-medium">
                                {cashFlowLabel(flow.event_type)}
                              </td>
                              <td className="px-3 py-3 text-sm text-muted-foreground">
                                {flow.note ?? "—"}
                              </td>
                              <td className="px-3 py-3 text-right text-sm tabular-nums">
                                {formatCurrency(flow.amount, { showSign: true })}
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  </CardContent>
                </Card>
              </TabsContent>

              <TabsContent value="dynamic" className="mt-4">
                {dynamicLoading ? (
                  <PositionTableSkeleton rows={8} />
                ) : dynamicError ? (
                  <Card>
                    <CardContent className="p-12 text-center">
                      <p className="text-destructive">Failed to load dynamic config history.</p>
                    </CardContent>
                  </Card>
                ) : (
                  <Card>
                    <CardHeader>
                      <CardTitle className="flex items-center gap-2">
                        <SlidersHorizontal className="h-5 w-5" />
                        Dynamic Config History
                      </CardTitle>
                    </CardHeader>
                    <CardContent>
                      <div className="space-y-3">
                        {dynamicHistory.map((entry) => (
                          <div key={entry.id} className="rounded-lg border p-4">
                            <div className="flex flex-wrap items-start justify-between gap-3">
                              <div>
                                <div className="flex items-center gap-2">
                                  <Badge variant="outline">{entry.action}</Badge>
                                  <span className="text-sm text-muted-foreground">
                                    {formatTimeAgo(entry.created_at)}
                                  </span>
                                </div>
                                <p className="mt-2 text-sm font-medium">
                                  {formatDynamicKey(entry.config_key)}
                                </p>
                                <p className="text-sm text-muted-foreground">
                                  {formatDynamicConfigValue(entry.config_key, entry.old_value)} →{" "}
                                  {formatDynamicConfigValue(entry.config_key, entry.new_value)}
                                </p>
                                {entry.reason && (
                                  <p className="mt-2 text-xs text-muted-foreground">
                                    {entry.reason}
                                  </p>
                                )}
                              </div>
                            </div>
                          </div>
                        ))}
                      </div>
                    </CardContent>
                  </Card>
                )}
              </TabsContent>
            </Tabs>
          </>
        )}
      </div>
    </ErrorBoundary>
  );
}
