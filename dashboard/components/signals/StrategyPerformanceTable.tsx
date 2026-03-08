"use client";

import { useState } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { InfoTooltip } from "@/components/shared/InfoTooltip";
import { useStrategyPerformanceQuery } from "@/hooks/queries/useSignalsQuery";
import { cn, formatWinRatePercent } from "@/lib/utils";

const STRATEGY_STYLES: Record<string, { label: string; className: string }> = {
  flow: {
    label: "Flow",
    className: "bg-blue-500/10 text-blue-600 border-blue-500/20",
  },
  cross_market: {
    label: "Cross Market",
    className: "bg-purple-500/10 text-purple-600 border-purple-500/20",
  },
  mean_reversion: {
    label: "Mean Reversion",
    className: "bg-amber-500/10 text-amber-600 border-amber-500/20",
  },
  resolution_proximity: {
    label: "Resolution",
    className: "bg-green-500/10 text-green-600 border-green-500/20",
  },
};

function formatPnl(value: number) {
  const sign = value >= 0 ? "+" : "";
  return `${sign}$${Math.abs(value).toFixed(2)}`;
}

export function StrategyPerformanceTable() {
  const [periodDays, setPeriodDays] = useState(7);
  const { data: strategies = [], isLoading } =
    useStrategyPerformanceQuery(periodDays);

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between">
        <CardTitle className="flex items-center gap-2">
          <span>Strategy Performance</span>
          <InfoTooltip content="This compares how each signal type has been performing. Net P&L is total profit or loss, win rate is the share of profitable resolved trades, and max drawdown shows the worst pullback during the period." />
        </CardTitle>
        <div className="flex gap-1">
          {[7, 30].map((d) => (
            <button
              key={d}
              onClick={() => setPeriodDays(d)}
              className={cn(
                "px-3 py-1 text-xs font-medium rounded-md transition-colors",
                periodDays === d
                  ? "bg-primary text-primary-foreground"
                  : "bg-muted text-muted-foreground hover:bg-muted/80",
              )}
            >
              {d}d
            </button>
          ))}
        </div>
      </CardHeader>
      <CardContent>
        {isLoading ? (
          <div className="space-y-3">
            {Array.from({ length: 4 }).map((_, i) => (
              <Skeleton key={i} className="h-10 w-full" />
            ))}
          </div>
        ) : strategies.length === 0 ? (
          <p className="text-sm text-muted-foreground text-center py-8">
            No strategy performance data yet. Signals will populate this table
            as they are generated and evaluated.
          </p>
        ) : (
          <div className="overflow-x-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Strategy</TableHead>
                  <TableHead className="text-right">Signals</TableHead>
                  <TableHead className="text-right">Executed</TableHead>
                  <TableHead className="text-right">Win Rate</TableHead>
                  <TableHead className="text-right">Net P&L</TableHead>
                  <TableHead className="text-right">Avg P&L</TableHead>
                  <TableHead className="text-right">Sharpe</TableHead>
                  <TableHead className="text-right">Max DD</TableHead>
                  <TableHead className="text-right">Avg Hold</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {strategies.map((s) => {
                  const style = STRATEGY_STYLES[s.strategy] || {
                    label: s.strategy,
                    className:
                      "bg-muted text-muted-foreground border-muted-foreground/20",
                  };
                  return (
                    <TableRow key={s.strategy}>
                      <TableCell>
                        <Badge
                          variant="outline"
                          className={cn("text-xs", style.className)}
                        >
                          {style.label}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {s.total_signals}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {s.executed}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {s.win_rate != null
                          ? formatWinRatePercent(s.win_rate, { input: "ratio" })
                          : "\u2014"}
                      </TableCell>
                      <TableCell
                        className={cn(
                          "text-right tabular-nums font-medium",
                          s.net_pnl >= 0 ? "text-profit" : "text-loss",
                        )}
                      >
                        {formatPnl(s.net_pnl)}
                      </TableCell>
                      <TableCell
                        className={cn(
                          "text-right tabular-nums",
                          s.avg_pnl >= 0 ? "text-profit" : "text-loss",
                        )}
                      >
                        {formatPnl(s.avg_pnl)}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {s.sharpe != null ? s.sharpe.toFixed(2) : "\u2014"}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {s.max_drawdown_pct != null
                          ? `${(s.max_drawdown_pct * 100).toFixed(1)}%`
                          : "\u2014"}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {s.avg_hold_hours != null
                          ? `${s.avg_hold_hours.toFixed(1)}h`
                          : "\u2014"}
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
