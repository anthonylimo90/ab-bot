"use client";

import { useMemo, useState } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { MetricCard } from "@/components/shared/MetricCard";
import { ErrorBoundary } from "@/components/shared/ErrorBoundary";
import { PositionTableSkeleton } from "@/components/shared/Skeletons";
import {
  useActivityHistoryQuery,
  useDynamicConfigHistoryQuery,
} from "@/hooks/queries/useHistoryQuery";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { formatCurrency, formatTimeAgo, cn } from "@/lib/utils";
import {
  History,
  ChevronLeft,
  ChevronRight,
  Copy,
  AlertCircle,
  XCircle,
  Activity as ActivityIcon,
  SlidersHorizontal,
} from "lucide-react";

const PAGE_SIZE = 50;

type ActivityFilter = "all" | "copied" | "skipped" | "failed";
type HistoryTab = "activity" | "dynamic";

const activityMeta = {
  TRADE_COPIED: {
    label: "Copied",
    icon: <Copy className="h-4 w-4 text-blue-500" />,
    badgeClass: "bg-blue-500/10 text-blue-600",
  },
  TRADE_COPY_SKIPPED: {
    label: "Skipped",
    icon: <AlertCircle className="h-4 w-4 text-yellow-500" />,
    badgeClass: "bg-yellow-500/10 text-yellow-600",
  },
  TRADE_COPY_FAILED: {
    label: "Failed",
    icon: <XCircle className="h-4 w-4 text-red-500" />,
    badgeClass: "bg-red-500/10 text-red-600",
  },
} as const;

function matchesFilter(type: string, filter: ActivityFilter): boolean {
  if (filter === "all") return true;
  if (filter === "copied") return type === "TRADE_COPIED";
  if (filter === "skipped") return type === "TRADE_COPY_SKIPPED";
  if (filter === "failed") return type === "TRADE_COPY_FAILED";
  return true;
}

function formatDynamicValue(value: number | null): string {
  if (value === null) return "-";
  return Number.isInteger(value) ? String(value) : value.toFixed(4);
}

function metricValue(metrics: Record<string, unknown> | null | undefined, key: string): string {
  if (!metrics) return "-";
  const raw = metrics[key];
  if (typeof raw !== "number") return "-";
  return raw.toFixed(4);
}

function formatDynamicKey(key: string | null): string {
  if (!key) return "(global)";
  if (key === "ARB_MONITOR_AGGRESSIVENESS_LEVEL") return "Opportunity Aggressiveness";
  if (key === "ARB_MONITOR_EXPLORATION_SLOTS") return "Exploration Slots";
  if (key === "ARB_MONITOR_MAX_MARKETS") return "Max Monitored Markets";
  if (key === "ARB_MIN_PROFIT_THRESHOLD") return "Min Net Profit Threshold";
  return key;
}

function formatDynamicHistoryValue(key: string | null, value: number | null): string {
  if (value === null) return "-";
  if (key === "ARB_MONITOR_AGGRESSIVENESS_LEVEL") {
    if (value <= 0.5) return "stable";
    if (value >= 1.5) return "discovery";
    return "balanced";
  }
  return formatDynamicValue(value);
}

function dynamicActionClass(action: string): string {
  if (action === "manual_update") return "bg-blue-500/10 text-blue-600 border-blue-500/20";
  if (action === "rollback") return "bg-yellow-500/10 text-yellow-700 border-yellow-500/20";
  if (action === "applied") return "bg-green-500/10 text-green-700 border-green-500/20";
  if (action === "recommended") return "bg-purple-500/10 text-purple-700 border-purple-500/20";
  return "border-border";
}

export default function HistoryPage() {
  const { currentWorkspace } = useWorkspaceStore();
  const workspaceId = currentWorkspace?.id;

  const [historyTab, setHistoryTab] = useState<HistoryTab>("activity");
  const [activityFilter, setActivityFilter] = useState<ActivityFilter>("all");
  const [activityPage, setActivityPage] = useState(0);
  const [dynamicPage, setDynamicPage] = useState(0);

  const {
    data: activity = [],
    isLoading,
    error,
  } = useActivityHistoryQuery({
    limit: PAGE_SIZE,
    offset: activityPage * PAGE_SIZE,
  });

  const {
    data: dynamicHistory = [],
    isLoading: isDynamicLoading,
    error: dynamicError,
  } = useDynamicConfigHistoryQuery({
    workspaceId,
    limit: PAGE_SIZE,
    offset: dynamicPage * PAGE_SIZE,
  });

  const filteredActivity = useMemo(
    () => activity.filter((item) => matchesFilter(item.type, activityFilter)),
    [activity, activityFilter],
  );

  const copiedCount = useMemo(
    () => filteredActivity.filter((item) => item.type === "TRADE_COPIED").length,
    [filteredActivity],
  );
  const skippedCount = useMemo(
    () =>
      filteredActivity.filter((item) => item.type === "TRADE_COPY_SKIPPED")
        .length,
    [filteredActivity],
  );
  const failedCount = useMemo(
    () =>
      filteredActivity.filter((item) => item.type === "TRADE_COPY_FAILED")
        .length,
    [filteredActivity],
  );
  const netPnl = useMemo(
    () =>
      filteredActivity.reduce((sum, item) => sum + Number(item.pnl ?? 0), 0),
    [filteredActivity],
  );

  return (
    <ErrorBoundary>
      <div className="space-y-5 sm:space-y-6">
        <div>
          <h1 className="flex items-center gap-2 text-2xl font-bold tracking-tight sm:text-3xl">
            <History className="h-8 w-8" />
            History
          </h1>
          <p className="text-muted-foreground">
            Copy-trade outcomes and dynamic runtime tuning changes
          </p>
        </div>

        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
          <MetricCard
            title="Net P&L / Slippage"
            value={formatCurrency(netPnl, { showSign: true })}
            trend={netPnl >= 0 ? "up" : "down"}
          />
          <MetricCard title="Copied" value={String(copiedCount)} trend="neutral" />
          <MetricCard title="Skipped" value={String(skippedCount)} trend="neutral" />
          <MetricCard
            title="Failed"
            value={String(failedCount)}
            trend={failedCount > 0 ? "down" : "neutral"}
          />
        </div>

        <Tabs
          value={historyTab}
          onValueChange={(value) => setHistoryTab(value as HistoryTab)}
        >
          <TabsList className="grid w-full max-w-md grid-cols-2">
            <TabsTrigger value="activity">Copy Activity</TabsTrigger>
            <TabsTrigger value="dynamic">Dynamic Config</TabsTrigger>
          </TabsList>

          <TabsContent value="activity" className="mt-4 space-y-4">
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
              <Select
                value={activityFilter}
                onValueChange={(value) => {
                  setActivityFilter(value as ActivityFilter);
                  setActivityPage(0);
                }}
              >
                <SelectTrigger className="w-full sm:w-[180px]">
                  <SelectValue placeholder="Activity Type" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">All Activity</SelectItem>
                  <SelectItem value="copied">Copied</SelectItem>
                  <SelectItem value="skipped">Skipped</SelectItem>
                  <SelectItem value="failed">Failed</SelectItem>
                </SelectContent>
              </Select>
            </div>

            {isLoading ? (
              <PositionTableSkeleton rows={10} />
            ) : error ? (
              <Card>
                <CardContent className="p-12 text-center">
                  <p className="text-destructive">Failed to load history.</p>
                </CardContent>
              </Card>
            ) : filteredActivity.length === 0 ? (
              <Card>
                <CardContent className="p-12 text-center">
                  <History className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
                  <h3 className="text-lg font-medium mb-2">No activity</h3>
                  <p className="text-muted-foreground">
                    Activity will appear here as copy trades are processed.
                  </p>
                </CardContent>
              </Card>
            ) : (
              <>
                <Card>
                  <CardHeader>
                    <CardTitle>Activity Log</CardTitle>
                  </CardHeader>
                  <CardContent>
                    <div className="overflow-x-auto">
                      <table className="w-full min-w-[680px]">
                        <thead className="border-b bg-muted/50">
                          <tr>
                            <th className="text-left p-4 font-medium">Type</th>
                            <th className="text-left p-4 font-medium">Message</th>
                            <th className="text-right p-4 font-medium">P&amp;L</th>
                            <th className="text-right p-4 font-medium">Time</th>
                          </tr>
                        </thead>
                        <tbody>
                          {filteredActivity.map((item) => {
                            const meta =
                              activityMeta[
                                item.type as keyof typeof activityMeta
                              ];

                            return (
                              <tr key={item.id} className="border-b hover:bg-muted/30">
                                <td className="p-4">
                                  <span
                                    className={cn(
                                      "inline-flex items-center gap-2 rounded px-2 py-1 text-xs font-medium",
                                      meta?.badgeClass ?? "bg-muted text-foreground",
                                    )}
                                  >
                                    {meta?.icon ?? <ActivityIcon className="h-4 w-4" />}
                                    {meta?.label ?? item.type}
                                  </span>
                                </td>
                                <td className="p-4 text-sm">
                                  <p className="break-words">{item.message}</p>
                                  {(item.skip_reason || item.error_message) && (
                                    <div className="mt-1 flex flex-wrap gap-1">
                                      {item.skip_reason && (
                                        <span className="rounded bg-yellow-500/10 px-1.5 py-0.5 text-xs text-yellow-700">
                                          skip: {item.skip_reason}
                                        </span>
                                      )}
                                      {item.error_message && (
                                        <span className="rounded bg-red-500/10 px-1.5 py-0.5 text-xs text-red-700">
                                          error: {item.error_message}
                                        </span>
                                      )}
                                    </div>
                                  )}
                                </td>
                                <td className="p-4 text-right">
                                  {item.pnl === undefined ? (
                                    <span className="text-muted-foreground">-</span>
                                  ) : (
                                    <span
                                      className={cn(
                                        "tabular-nums font-medium",
                                        item.pnl >= 0 ? "text-profit" : "text-loss",
                                      )}
                                    >
                                      {formatCurrency(item.pnl, { showSign: true })}
                                    </span>
                                  )}
                                </td>
                                <td className="p-4 text-right text-muted-foreground text-sm">
                                  <div>{new Date(item.created_at).toLocaleString()}</div>
                                  <div>{formatTimeAgo(item.created_at)}</div>
                                </td>
                              </tr>
                            );
                          })}
                        </tbody>
                      </table>
                    </div>
                  </CardContent>
                </Card>

                <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                  <p className="text-sm text-muted-foreground">
                    Page {activityPage + 1} &middot; {filteredActivity.length} records
                  </p>
                  <div className="flex gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={activityPage === 0}
                      onClick={() => setActivityPage((page) => page - 1)}
                    >
                      <ChevronLeft className="h-4 w-4 mr-1" />
                      Prev
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={activity.length < PAGE_SIZE}
                      onClick={() => setActivityPage((page) => page + 1)}
                    >
                      Next
                      <ChevronRight className="h-4 w-4 ml-1" />
                    </Button>
                  </div>
                </div>
              </>
            )}
          </TabsContent>

          <TabsContent value="dynamic" className="mt-4 space-y-4">
            {!workspaceId ? (
              <Card>
                <CardContent className="p-6 text-sm text-muted-foreground">
                  Select a workspace to view dynamic tuning history.
                </CardContent>
              </Card>
            ) : isDynamicLoading ? (
              <PositionTableSkeleton rows={8} />
            ) : dynamicError ? (
              <Card>
                <CardContent className="p-6 text-destructive">
                  Failed to load dynamic config history.
                </CardContent>
              </Card>
            ) : dynamicHistory.length === 0 ? (
              <Card>
                <CardContent className="p-10 text-center text-muted-foreground">
                  No dynamic tuner history yet.
                </CardContent>
              </Card>
            ) : (
              <>
                <Card>
                  <CardHeader>
                    <CardTitle className="flex items-center gap-2">
                      <SlidersHorizontal className="h-4 w-4" />
                      Dynamic Config Changes
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <div className="overflow-x-auto">
                      <table className="w-full min-w-[920px]">
                        <thead className="border-b bg-muted/50">
                          <tr>
                            <th className="p-3 text-left text-xs font-medium">Key</th>
                            <th className="p-3 text-left text-xs font-medium">Action</th>
                            <th className="p-3 text-left text-xs font-medium">Old → New</th>
                            <th className="p-3 text-left text-xs font-medium">Reason</th>
                            <th className="p-3 text-left text-xs font-medium">Snapshot</th>
                            <th className="p-3 text-left text-xs font-medium">Outcome</th>
                            <th className="p-3 text-right text-xs font-medium">Time</th>
                          </tr>
                        </thead>
                        <tbody>
                          {dynamicHistory.map((entry) => (
                            <tr key={entry.id} className="border-b align-top hover:bg-muted/20">
                              <td className="p-3 font-mono text-xs">
                                {formatDynamicKey(entry.config_key)}
                              </td>
                              <td className="p-3">
                                <span
                                  className={cn(
                                    "rounded border px-2 py-0.5 text-xs",
                                    dynamicActionClass(entry.action),
                                  )}
                                >
                                  {entry.action}
                                </span>
                              </td>
                              <td className="p-3 text-xs tabular-nums">
                                {formatDynamicHistoryValue(entry.config_key, entry.old_value)} →{" "}
                                {formatDynamicHistoryValue(entry.config_key, entry.new_value)}
                              </td>
                              <td className="max-w-[280px] p-3 text-xs break-words">{entry.reason}</td>
                              <td className="p-3 text-xs text-muted-foreground">
                                fill={metricValue(entry.metrics_snapshot, "successful_fill_rate")}
                                <br />
                                slip_p90={metricValue(entry.metrics_snapshot, "realized_slippage_p90")}
                                <br />
                                pnl={metricValue(entry.metrics_snapshot, "recent_pnl")}
                              </td>
                              <td className="p-3 text-xs text-muted-foreground">
                                fill={metricValue(entry.outcome_metrics, "successful_fill_rate")}
                                <br />
                                pnl={metricValue(entry.outcome_metrics, "recent_pnl")}
                                <br />
                                dd={metricValue(entry.outcome_metrics, "recent_drawdown")}
                              </td>
                              <td className="p-3 text-right text-xs text-muted-foreground">
                                <div>{new Date(entry.created_at).toLocaleString()}</div>
                                <div>{formatTimeAgo(entry.created_at)}</div>
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  </CardContent>
                </Card>

                <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                  <p className="text-sm text-muted-foreground">
                    Page {dynamicPage + 1} &middot; {dynamicHistory.length} records
                  </p>
                  <div className="flex gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={dynamicPage === 0}
                      onClick={() => setDynamicPage((page) => page - 1)}
                    >
                      <ChevronLeft className="h-4 w-4 mr-1" />
                      Prev
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={dynamicHistory.length < PAGE_SIZE}
                      onClick={() => setDynamicPage((page) => page + 1)}
                    >
                      Next
                      <ChevronRight className="h-4 w-4 ml-1" />
                    </Button>
                  </div>
                </div>
              </>
            )}
          </TabsContent>
        </Tabs>
      </div>
    </ErrorBoundary>
  );
}
