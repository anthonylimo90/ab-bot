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
import { useClosedPositionsQuery } from "@/hooks/queries/useHistoryQuery";
import {
  formatCurrency,
  formatPercent,
  shortenAddress,
  cn,
} from "@/lib/utils";
import { History, ChevronLeft, ChevronRight } from "lucide-react";

const PAGE_SIZE = 50;

export default function HistoryPage() {
  const [outcomeFilter, setOutcomeFilter] = useState<"all" | "yes" | "no">(
    "all",
  );
  const [sourceFilter, setSourceFilter] = useState<
    "all" | "copy" | "manual"
  >("all");
  const [page, setPage] = useState(0);

  const {
    data: positions = [],
    isLoading,
    error,
  } = useClosedPositionsQuery({
    outcome: outcomeFilter === "all" ? undefined : outcomeFilter,
    copyTradesOnly: sourceFilter === "copy" ? true : undefined,
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
  });

  // Client-side filter for "manual" since API only has copy_trades_only=true
  const filteredPositions = useMemo(() => {
    if (sourceFilter === "manual") {
      return positions.filter((p) => !p.is_copy_trade);
    }
    return positions;
  }, [positions, sourceFilter]);

  // Summary stats
  const totalRealizedPnl = useMemo(
    () =>
      filteredPositions.reduce(
        (sum, p) => sum + (p.realized_pnl ?? p.unrealized_pnl),
        0,
      ),
    [filteredPositions],
  );
  const winners = useMemo(
    () =>
      filteredPositions.filter((p) => (p.realized_pnl ?? p.unrealized_pnl) > 0)
        .length,
    [filteredPositions],
  );
  const winRate =
    filteredPositions.length > 0
      ? (winners / filteredPositions.length) * 100
      : 0;
  const avgPnl =
    filteredPositions.length > 0
      ? totalRealizedPnl / filteredPositions.length
      : 0;
  const copyTrades = filteredPositions.filter((p) => p.is_copy_trade).length;

  return (
    <ErrorBoundary>
      <div className="space-y-6">
        {/* Header */}
        <div>
          <h1 className="text-3xl font-bold tracking-tight flex items-center gap-2">
            <History className="h-8 w-8" />
            History
          </h1>
          <p className="text-muted-foreground">
            Closed positions and realized performance
          </p>
        </div>

        {/* Summary MetricCards */}
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
          <MetricCard
            title="Total Realized P&L"
            value={formatCurrency(totalRealizedPnl, { showSign: true })}
            trend={totalRealizedPnl >= 0 ? "up" : "down"}
          />
          <MetricCard
            title="Win Rate"
            value={formatPercent(winRate)}
            trend={winRate >= 50 ? "up" : "down"}
            changeLabel={`${winners} of ${filteredPositions.length} trades`}
          />
          <MetricCard
            title="Avg P&L per Trade"
            value={formatCurrency(avgPnl, { showSign: true })}
            trend={avgPnl >= 0 ? "up" : "down"}
          />
          <MetricCard
            title="Copy Trades"
            value={String(copyTrades)}
            changeLabel={`of ${filteredPositions.length} total`}
            trend="neutral"
          />
        </div>

        {/* Filters */}
        <div className="flex items-center gap-2">
          <Select
            value={outcomeFilter}
            onValueChange={(v) => {
              setOutcomeFilter(v as typeof outcomeFilter);
              setPage(0);
            }}
          >
            <SelectTrigger className="w-[140px]">
              <SelectValue placeholder="Outcome" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All Outcomes</SelectItem>
              <SelectItem value="yes">YES</SelectItem>
              <SelectItem value="no">NO</SelectItem>
            </SelectContent>
          </Select>
          <Select
            value={sourceFilter}
            onValueChange={(v) => {
              setSourceFilter(v as typeof sourceFilter);
              setPage(0);
            }}
          >
            <SelectTrigger className="w-[140px]">
              <SelectValue placeholder="Source" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All Sources</SelectItem>
              <SelectItem value="copy">Copy Trades</SelectItem>
              <SelectItem value="manual">Manual</SelectItem>
            </SelectContent>
          </Select>
        </div>

        {/* Table */}
        {isLoading ? (
          <PositionTableSkeleton rows={10} />
        ) : error ? (
          <Card>
            <CardContent className="p-12 text-center">
              <p className="text-destructive">Failed to load history.</p>
            </CardContent>
          </Card>
        ) : filteredPositions.length === 0 ? (
          <Card>
            <CardContent className="p-12 text-center">
              <History className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
              <h3 className="text-lg font-medium mb-2">
                No closed positions
              </h3>
              <p className="text-muted-foreground">
                Your trade history will appear here.
              </p>
            </CardContent>
          </Card>
        ) : (
          <>
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center justify-between">
                  <span>Closed Positions</span>
                  <span
                    className={cn(
                      "text-lg font-bold",
                      totalRealizedPnl >= 0 ? "text-profit" : "text-loss",
                    )}
                  >
                    Total:{" "}
                    {formatCurrency(totalRealizedPnl, { showSign: true })}
                  </span>
                </CardTitle>
              </CardHeader>
              <CardContent>
                <div className="overflow-x-auto">
                  <table className="w-full">
                    <thead className="border-b bg-muted/50">
                      <tr>
                        <th className="text-left p-4 font-medium">Market</th>
                        <th className="text-left p-4 font-medium">Outcome</th>
                        <th className="text-right p-4 font-medium">Entry</th>
                        <th className="text-right p-4 font-medium">Exit</th>
                        <th className="text-right p-4 font-medium">Size</th>
                        <th className="text-right p-4 font-medium">
                          Realized P&L
                        </th>
                        <th className="text-right p-4 font-medium">Source</th>
                        <th className="text-right p-4 font-medium">Closed</th>
                      </tr>
                    </thead>
                    <tbody>
                      {filteredPositions.map((p) => {
                        const pnl = p.realized_pnl ?? p.unrealized_pnl;
                        return (
                          <tr
                            key={p.id}
                            className="border-b hover:bg-muted/30"
                          >
                            <td className="p-4">
                              <p className="font-medium text-sm">
                                {p.market_id}
                              </p>
                            </td>
                            <td className="p-4">
                              <span
                                className={cn(
                                  "px-2 py-1 rounded text-xs font-medium uppercase",
                                  p.outcome === "yes"
                                    ? "bg-profit/10 text-profit"
                                    : "bg-loss/10 text-loss",
                                )}
                              >
                                {p.outcome}
                              </span>
                            </td>
                            <td className="p-4 text-right tabular-nums">
                              ${p.entry_price.toFixed(2)}
                            </td>
                            <td className="p-4 text-right tabular-nums">
                              ${p.current_price.toFixed(2)}
                            </td>
                            <td className="p-4 text-right tabular-nums">
                              {formatCurrency(p.quantity * p.entry_price)}
                            </td>
                            <td className="p-4 text-right">
                              <span
                                className={cn(
                                  "tabular-nums font-medium",
                                  pnl >= 0 ? "text-profit" : "text-loss",
                                )}
                              >
                                {formatCurrency(pnl, { showSign: true })}
                              </span>
                            </td>
                            <td className="p-4 text-right text-muted-foreground text-sm">
                              {p.source_wallet
                                ? shortenAddress(p.source_wallet)
                                : "Manual"}
                            </td>
                            <td className="p-4 text-right text-muted-foreground text-sm">
                              {new Date(p.updated_at).toLocaleDateString()}
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
              </CardContent>
            </Card>

            {/* Pagination */}
            <div className="flex items-center justify-between">
              <p className="text-sm text-muted-foreground">
                Page {page + 1} &middot; {filteredPositions.length} records
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
                  disabled={positions.length < PAGE_SIZE}
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
