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
import { MetricCard } from "@/components/shared/MetricCard";
import { ErrorBoundary } from "@/components/shared/ErrorBoundary";
import { PositionTableSkeleton } from "@/components/shared/Skeletons";
import { useActivityHistoryQuery } from "@/hooks/queries/useHistoryQuery";
import { formatCurrency, formatTimeAgo, cn } from "@/lib/utils";
import {
  History,
  ChevronLeft,
  ChevronRight,
  Copy,
  AlertCircle,
  XCircle,
  Activity as ActivityIcon,
} from "lucide-react";

const PAGE_SIZE = 50;

type ActivityFilter = "all" | "copied" | "skipped" | "failed";

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

export default function HistoryPage() {
  const [activityFilter, setActivityFilter] = useState<ActivityFilter>("all");
  const [page, setPage] = useState(0);

  const {
    data: activity = [],
    isLoading,
    error,
  } = useActivityHistoryQuery({
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
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
      filteredActivity.reduce((sum, item) => sum + (item.pnl ?? 0), 0),
    [filteredActivity],
  );

  return (
    <ErrorBoundary>
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight flex items-center gap-2">
            <History className="h-8 w-8" />
            History
          </h1>
          <p className="text-muted-foreground">
            Persisted copy-trade activity and outcomes
          </p>
        </div>

        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
          <MetricCard
            title="Net P&L / Slippage"
            value={formatCurrency(netPnl, { showSign: true })}
            trend={netPnl >= 0 ? "up" : "down"}
          />
          <MetricCard
            title="Copied"
            value={String(copiedCount)}
            trend="neutral"
          />
          <MetricCard
            title="Skipped"
            value={String(skippedCount)}
            trend="neutral"
          />
          <MetricCard
            title="Failed"
            value={String(failedCount)}
            trend={failedCount > 0 ? "down" : "neutral"}
          />
        </div>

        <div className="flex items-center gap-2">
          <Select
            value={activityFilter}
            onValueChange={(v) => {
              setActivityFilter(v as ActivityFilter);
              setPage(0);
            }}
          >
            <SelectTrigger className="w-[180px]">
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
                  <table className="w-full">
                    <thead className="border-b bg-muted/50">
                      <tr>
                        <th className="text-left p-4 font-medium">Type</th>
                        <th className="text-left p-4 font-medium">Message</th>
                        <th className="text-right p-4 font-medium">P&L</th>
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
                            <td className="p-4 text-sm">{item.message}</td>
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

            <div className="flex items-center justify-between">
              <p className="text-sm text-muted-foreground">
                Page {page + 1} &middot; {filteredActivity.length} records
              </p>
              <div className="flex gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={page === 0}
                  onClick={() => setPage((p) => p - 1)}
                >
                  <ChevronLeft className="h-4 w-4 mr-1" />
                  Prev
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={activity.length < PAGE_SIZE}
                  onClick={() => setPage((p) => p + 1)}
                >
                  Next
                  <ChevronRight className="h-4 w-4 ml-1" />
                </Button>
              </div>
            </div>
          </>
        )}
      </div>
    </ErrorBoundary>
  );
}
