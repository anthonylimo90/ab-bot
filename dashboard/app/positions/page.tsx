"use client";

import { useState, useCallback, useMemo } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
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
import { MetricCard } from "@/components/shared/MetricCard";
import { LiveIndicator } from "@/components/shared/LiveIndicator";
import { ErrorBoundary } from "@/components/shared/ErrorBoundary";
import { InfoTooltip } from "@/components/shared/InfoTooltip";
import { PageIntro } from "@/components/shared/PageIntro";
import {
  useOpenPositions,
  usePositionsQuery,
  usePositionsSummaryQuery,
  useClosePositionMutation,
} from "@/hooks/queries/usePositionsQuery";
import {
  useWebSocket,
  useBatchedPositionUpdates,
} from "@/hooks/useWebSocket";
import { useQueryClient } from "@tanstack/react-query";
import { queryKeys } from "@/lib/queryClient";
import { formatCurrency, formatPercent, cn } from "@/lib/utils";
import {
  Layers,
  ChevronLeft,
  ChevronRight,
} from "lucide-react";
import type { Position, PositionState, WebSocketMessage } from "@/types/api";

type TabValue = "open" | "closed";

const STATE_BADGE_STYLES: Record<
  PositionState,
  { label: string; className: string }
> = {
  pending: {
    label: "Pending",
    className: "bg-yellow-500/10 text-yellow-600 border-yellow-500/20",
  },
  open: {
    label: "Open",
    className: "bg-green-500/10 text-green-600 border-green-500/20",
  },
  exit_ready: {
    label: "Exit Ready",
    className: "bg-blue-500/10 text-blue-600 border-blue-500/20",
  },
  closing: {
    label: "Closing",
    className: "bg-orange-500/10 text-orange-600 border-orange-500/20",
  },
  closed: {
    label: "Closed",
    className: "bg-muted text-muted-foreground border-muted-foreground/20",
  },
  entry_failed: {
    label: "Entry Failed",
    className: "bg-red-500/10 text-red-600 border-red-500/20",
  },
  exit_failed: {
    label: "Exit Failed",
    className: "bg-red-500/10 text-red-600 border-red-500/20",
  },
  stalled: {
    label: "Stalled",
    className: "bg-purple-500/10 text-purple-600 border-purple-500/20",
  },
};

function StateBadge({ state }: { state?: PositionState }) {
  const s = state ?? "open";
  const style = STATE_BADGE_STYLES[s] ?? STATE_BADGE_STYLES.open;
  return (
    <Badge variant="outline" className={cn("text-xs", style.className)}>
      {style.label}
    </Badge>
  );
}

function formatDuration(openedAt: string, closedAt?: string): string {
  const start = new Date(openedAt).getTime();
  const end = closedAt ? new Date(closedAt).getTime() : Date.now();
  const diffMs = end - start;
  const hours = Math.floor(diffMs / 3600000);
  const mins = Math.floor((diffMs % 3600000) / 60000);
  if (hours >= 24) {
    const days = Math.floor(hours / 24);
    return `${days}d ${hours % 24}h`;
  }
  return hours > 0 ? `${hours}h ${mins}m` : `${mins}m`;
}

const PAGE_SIZE = 25;

export default function PositionsPage() {
  const [activeTab, setActiveTab] = useState<TabValue>("open");
  const [closedPage, setClosedPage] = useState(0);

  // Open positions data
  const { openPositions, totalUnrealizedPnl, isLoading: openLoading } =
    useOpenPositions();
  const { data: positionsSummary } = usePositionsSummaryQuery();

  // Closed positions data (paginated)
  const { data: closedPositions = [], isLoading: closedLoading } =
    usePositionsQuery({ status: "closed", limit: 500 });

  const closeMutation = useClosePositionMutation();
  const queryClient = useQueryClient();

  // WebSocket for real-time position updates
  const handleBatchedUpdate = useCallback(
    (acc: {
      updates: Map<string, { price: number; pnl: number; quantity: number }>;
      opened: string[];
      closed: string[];
    }) => {
      // If positions were opened or closed, refetch
      if (acc.opened.length > 0 || acc.closed.length > 0) {
        queryClient.invalidateQueries({ queryKey: queryKeys.positions.all() });
        return;
      }

      // For price updates, optimistically update the cache
      if (acc.updates.size > 0) {
        queryClient.setQueryData<Position[]>(
          queryKeys.positions.list({ status: "open", limit: 500 }),
          (old) => {
            if (!old) return old;
            return old.map((pos) => {
              const update = acc.updates.get(pos.id);
              if (!update) return pos;
              return {
                ...pos,
                current_price: update.price ?? pos.current_price,
                unrealized_pnl: update.pnl ?? pos.unrealized_pnl,
              };
            });
          },
        );
      }
    },
    [queryClient],
  );

  const { addUpdate } = useBatchedPositionUpdates(handleBatchedUpdate);

  const handleWsMessage = useCallback(
    (msg: WebSocketMessage) => {
      if (msg.type === "Position") {
        addUpdate(msg.data.position_id, msg.data.update_type, {
          price: msg.data.current_price,
          pnl: msg.data.unrealized_pnl,
          quantity: msg.data.quantity,
        });
      }
    },
    [addUpdate],
  );

  const { status: wsStatus } = useWebSocket({
    channel: "positions",
    onMessage: handleWsMessage,
  });

  // Derived stats
  const loadedClosedCount = closedPositions.length;
  const closedCount = positionsSummary?.closed_positions ?? loadedClosedCount;
  const winCount =
    positionsSummary?.wins ??
    closedPositions.filter((p) => (p.realized_pnl ?? 0) > 0).length;
  const winRate =
    positionsSummary?.win_rate ??
    (closedCount > 0 ? (winCount / closedCount) * 100 : 0);
  const summaryUnrealizedPnl =
    positionsSummary?.unrealized_pnl ?? totalUnrealizedPnl;
  const summaryOpenPositions =
    positionsSummary?.open_positions ?? openPositions.length;

  // Paginated closed positions
  const paginatedClosed = useMemo(() => {
    const start = closedPage * PAGE_SIZE;
    return closedPositions.slice(start, start + PAGE_SIZE);
  }, [closedPositions, closedPage]);
  const totalClosedPages = Math.max(1, Math.ceil(loadedClosedCount / PAGE_SIZE));

  const isLoading = activeTab === "open" ? openLoading : closedLoading;
  const positions = activeTab === "open" ? openPositions : paginatedClosed;

  return (
    <ErrorBoundary>
      <div className="space-y-5 sm:space-y-6">
        {/* Header */}
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center gap-3">
            <Layers className="h-6 w-6 text-muted-foreground" />
            <div>
              <h1 className="text-2xl font-bold">Positions</h1>
              <p className="text-sm text-muted-foreground">
                Manage open and closed positions
              </p>
            </div>
          </div>
          <LiveIndicator
            label={wsStatus === "connected" ? "LIVE" : "CONNECTING"}
          />
        </div>

        <PageIntro
          title="How to read positions"
          description="Positions are trades the system has already entered. Open positions are still active, while closed positions show completed outcomes."
          bullets={[
            "Unrealized P&L is the profit or loss if the open trade were closed right now.",
            "Win rate is based on closed trades only, so it may change as more positions finish.",
            "State tells you where a position is in its lifecycle, such as open, closing, or failed."
          ]}
        />

        {/* Metric Cards */}
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
          <MetricCard
            title="Unrealized P&L"
            value={formatCurrency(summaryUnrealizedPnl, { showSign: true })}
            trend={
              summaryUnrealizedPnl > 0
                ? "up"
                : summaryUnrealizedPnl < 0
                  ? "down"
                  : "neutral"
            }
          />
          <MetricCard
            title="Open Positions"
            value={String(summaryOpenPositions)}
            trend="neutral"
          />
          <MetricCard
            title="Closed Count"
            value={String(closedCount)}
            trend="neutral"
          />
          <MetricCard
            title="Win Rate"
            value={formatPercent(winRate)}
            trend={winRate >= 50 ? "up" : winRate > 0 ? "down" : "neutral"}
            changeLabel={`${winCount}W / ${closedCount - winCount}L`}
          />
        </div>

        {/* Tabs */}
        <Tabs
          value={activeTab}
          onValueChange={(v) => {
            setActiveTab(v as TabValue);
            setClosedPage(0);
          }}
        >
          <div className="overflow-x-auto pb-1">
            <TabsList className="w-max min-w-full sm:min-w-0">
              <TabsTrigger value="open">
                Open ({openPositions.length})
              </TabsTrigger>
              <TabsTrigger value="closed">Closed ({closedCount})</TabsTrigger>
            </TabsList>
          </div>
        </Tabs>

        {/* Positions Table */}
        <Card>
          <CardContent className="p-0">
            <div className="overflow-x-auto pb-1">
              <table className="w-full min-w-[860px] text-sm">
                <thead>
                  <tr className="border-b bg-muted/50">
                    <th className="px-4 py-3 text-left font-medium text-muted-foreground">
                      Market
                    </th>
                    <th className="px-4 py-3 text-left font-medium text-muted-foreground">
                      Side
                    </th>
                    <th className="px-4 py-3 text-left font-medium text-muted-foreground">
                      <span className="inline-flex items-center gap-1">
                        State
                        <InfoTooltip content="State explains where the position currently is in its lifecycle, such as active, waiting to close, closed, or failed." />
                      </span>
                    </th>
                    <th className="px-4 py-3 text-right font-medium text-muted-foreground">
                      Qty
                    </th>
                    <th className="px-4 py-3 text-right font-medium text-muted-foreground">
                      Entry
                    </th>
                    <th className="px-4 py-3 text-right font-medium text-muted-foreground">
                      {activeTab === "open" ? "Current" : "Exit"}
                    </th>
                    <th className="px-4 py-3 text-right font-medium text-muted-foreground">
                      {activeTab === "open" ? "Unrealized P&L" : "Realized P&L"}
                    </th>
                    {activeTab === "open" && (
                      <th className="px-4 py-3 text-right font-medium text-muted-foreground">
                        Source
                      </th>
                    )}
                    {activeTab === "closed" && (
                      <th className="px-4 py-3 text-right font-medium text-muted-foreground">
                        Duration
                      </th>
                    )}
                    {activeTab === "open" && (
                      <th className="px-4 py-3 text-center font-medium text-muted-foreground">
                        Close
                      </th>
                    )}
                  </tr>
                </thead>
                <tbody>
                  {isLoading ? (
                    Array.from({ length: 5 }).map((_, i) => (
                      <tr key={i} className="border-b">
                        {Array.from({
                          length: activeTab === "open" ? 9 : 8,
                        }).map((_, j) => (
                          <td key={j} className="px-4 py-3">
                            <div className="h-4 w-full animate-pulse rounded bg-muted" />
                          </td>
                        ))}
                      </tr>
                    ))
                  ) : positions.length === 0 ? (
                    <tr>
                      <td
                        colSpan={activeTab === "open" ? 9 : 8}
                        className="px-4 py-12 text-center text-muted-foreground"
                      >
                        No {activeTab} positions
                      </td>
                    </tr>
                  ) : (
                    positions.map((pos) => {
                      const pnl =
                        activeTab === "open"
                          ? pos.unrealized_pnl
                          : (pos.realized_pnl ?? 0);
                      const exitPrice =
                        activeTab === "closed"
                          ? (pos.yes_exit_price ?? pos.no_exit_price ?? pos.current_price)
                          : pos.current_price;
                      return (
                        <tr
                          key={pos.id}
                          className="border-b hover:bg-muted/30 transition-colors"
                        >
                          <td className="px-4 py-3">
                            <span
                              className="max-w-[180px] truncate block font-mono text-xs"
                              title={pos.market_id}
                            >
                              {pos.market_id.slice(0, 12)}...
                            </span>
                          </td>
                          <td className="px-4 py-3">
                            <Badge
                              variant="outline"
                              className={cn(
                                "text-xs",
                                pos.outcome === "yes"
                                  ? "bg-green-500/10 text-green-600 border-green-500/20"
                                  : "bg-red-500/10 text-red-600 border-red-500/20",
                              )}
                            >
                              {pos.outcome.toUpperCase()}
                            </Badge>
                          </td>
                          <td className="px-4 py-3">
                            <StateBadge state={pos.state} />
                          </td>
                          <td className="px-4 py-3 text-right tabular-nums">
                            {pos.quantity.toFixed(2)}
                          </td>
                          <td className="px-4 py-3 text-right tabular-nums">
                            {pos.entry_price.toFixed(4)}
                          </td>
                          <td className="px-4 py-3 text-right tabular-nums">
                            {exitPrice.toFixed(4)}
                          </td>
                          <td
                            className={cn(
                              "px-4 py-3 text-right font-medium tabular-nums",
                              pnl > 0
                                ? "text-profit"
                                : pnl < 0
                                  ? "text-loss"
                                  : "text-muted-foreground",
                            )}
                          >
                            {formatCurrency(pnl, { showSign: true })}
                          </td>
                          {activeTab === "open" && (
                            <td className="px-4 py-3 text-right">
                              {pos.source_wallet ? (
                                <span
                                  className="font-mono text-xs text-muted-foreground"
                                  title={pos.source_wallet}
                                >
                                  {pos.source_wallet.slice(0, 6)}...
                                </span>
                              ) : (
                                <span className="text-xs text-muted-foreground">
                                  arb
                                </span>
                              )}
                            </td>
                          )}
                          {activeTab === "closed" && (
                            <td className="px-4 py-3 text-right text-xs text-muted-foreground tabular-nums">
                              {formatDuration(pos.opened_at, pos.updated_at)}
                            </td>
                          )}
                          {activeTab === "open" && (
                            <td className="px-4 py-3 text-center">
                              <AlertDialog>
                                <AlertDialogTrigger asChild>
                                  <Button
                                    variant="ghost"
                                    size="sm"
                                    className="h-7 text-xs text-loss hover:text-loss hover:bg-loss/10"
                                    disabled={closeMutation.isPending}
                                  >
                                    Close
                                  </Button>
                                </AlertDialogTrigger>
                                <AlertDialogContent>
                                  <AlertDialogHeader>
                                    <AlertDialogTitle>
                                      Close Position?
                                    </AlertDialogTitle>
                                    <AlertDialogDescription>
                                      This will close your {pos.outcome.toUpperCase()}{" "}
                                      position ({pos.quantity.toFixed(2)} shares) at
                                      market price. Current P&L:{" "}
                                      {formatCurrency(pos.unrealized_pnl, {
                                        showSign: true,
                                      })}
                                    </AlertDialogDescription>
                                  </AlertDialogHeader>
                                  <AlertDialogFooter>
                                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                                    <AlertDialogAction
                                      onClick={() =>
                                        closeMutation.mutate({
                                          positionId: pos.id,
                                        })
                                      }
                                      className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                                    >
                                      Close Position
                                    </AlertDialogAction>
                                  </AlertDialogFooter>
                                </AlertDialogContent>
                              </AlertDialog>
                            </td>
                          )}
                        </tr>
                      );
                    })
                  )}
                </tbody>
              </table>
            </div>

            {/* Pagination for closed tab */}
            {activeTab === "closed" && closedCount > PAGE_SIZE && (
              <div className="flex flex-col gap-2 border-t px-4 py-3 sm:flex-row sm:items-center sm:justify-between">
                <span className="text-sm text-muted-foreground">
                  Page {closedPage + 1} of {totalClosedPages}
                </span>
                <div className="flex gap-1">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setClosedPage((p) => Math.max(0, p - 1))}
                    disabled={closedPage === 0}
                  >
                    <ChevronLeft className="h-4 w-4" />
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      setClosedPage((p) =>
                        Math.min(totalClosedPages - 1, p + 1),
                      )
                    }
                    disabled={closedPage >= totalClosedPages - 1}
                  >
                    <ChevronRight className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </ErrorBoundary>
  );
}
