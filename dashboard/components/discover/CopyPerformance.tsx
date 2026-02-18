"use client";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useCopyPerformanceQuery } from "@/hooks/queries/useDiscoverQuery";
import { AlertTriangle, CheckCircle, Info, TrendingDown, TrendingUp } from "lucide-react";
import { cn, formatCurrency } from "@/lib/utils";

interface CopyPerformanceProps {
  address: string;
  className?: string;
}

export function CopyPerformance({ address, className }: CopyPerformanceProps) {
  const { data, isLoading, isError } = useCopyPerformanceQuery(address);

  if (isLoading) {
    return (
      <Card className={className}>
        <CardHeader>
          <CardTitle className="text-sm">Copy Trade Performance</CardTitle>
        </CardHeader>
        <CardContent>
          <Skeleton className="h-24 w-full" />
        </CardContent>
      </Card>
    );
  }

  if (isError || !data) {
    return null;
  }

  if (data.copy_trade_count === 0) {
    return (
      <Card className={className}>
        <CardHeader>
          <CardTitle className="text-sm flex items-center gap-2">
            Copy Trade Performance
            <TooltipProvider>
              <Tooltip>
                <TooltipTrigger>
                  <Info className="h-3.5 w-3.5 text-muted-foreground" />
                </TooltipTrigger>
                <TooltipContent className="max-w-xs">
                  <p>
                    Compares the wallet&apos;s reported metrics against actual
                    copy trade outcomes to detect performance divergence.
                  </p>
                </TooltipContent>
              </Tooltip>
            </TooltipProvider>
          </CardTitle>
        </CardHeader>
        <CardContent className="text-center py-4">
          <p className="text-sm text-muted-foreground">
            No copy trades recorded yet for this wallet.
          </p>
        </CardContent>
      </Card>
    );
  }

  const copyWinRate = data.copy_win_rate ?? 0;
  const pnlPositive = data.copy_total_pnl >= 0;

  return (
    <Card className={className}>
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between">
          <CardTitle className="text-sm flex items-center gap-2">
            Copy Trade Performance
            <TooltipProvider>
              <Tooltip>
                <TooltipTrigger>
                  <Info className="h-3.5 w-3.5 text-muted-foreground" />
                </TooltipTrigger>
                <TooltipContent className="max-w-xs">
                  <p>
                    Compares reported win rate against actual copy trade
                    outcomes. Large divergence may indicate the wallet performs
                    differently when copied.
                  </p>
                </TooltipContent>
              </Tooltip>
            </TooltipProvider>
          </CardTitle>
          {data.has_significant_divergence ? (
            <Badge variant="destructive" className="text-xs gap-1">
              <AlertTriangle className="h-3 w-3" />
              High Divergence
            </Badge>
          ) : (
            <Badge
              variant="outline"
              className="text-xs gap-1 bg-green-500/10 text-green-600 border-green-500/20"
            >
              <CheckCircle className="h-3 w-3" />
              Aligned
            </Badge>
          )}
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
        {/* Win Rate Comparison */}
        <div className="space-y-2">
          <div className="flex items-center justify-between text-sm">
            <span className="text-muted-foreground">Reported Win Rate</span>
            <span className="font-medium tabular-nums">
              {data.reported_win_rate.toFixed(1)}%
            </span>
          </div>
          <div className="flex items-center justify-between text-sm">
            <span className="text-muted-foreground">Copy Win Rate</span>
            <span className="font-medium tabular-nums">
              {copyWinRate.toFixed(1)}%
            </span>
          </div>

          {/* Visual bar comparison */}
          <div className="space-y-1">
            <div className="flex items-center gap-2">
              <span className="text-[10px] text-muted-foreground w-16">
                Reported
              </span>
              <div className="flex-1 bg-muted rounded-full h-2">
                <div
                  className="bg-blue-500 h-2 rounded-full transition-all"
                  style={{ width: `${Math.min(data.reported_win_rate, 100)}%` }}
                />
              </div>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-[10px] text-muted-foreground w-16">
                Actual
              </span>
              <div className="flex-1 bg-muted rounded-full h-2">
                <div
                  className={cn(
                    "h-2 rounded-full transition-all",
                    data.has_significant_divergence
                      ? "bg-red-500"
                      : "bg-green-500",
                  )}
                  style={{ width: `${Math.min(copyWinRate, 100)}%` }}
                />
              </div>
            </div>
          </div>

          {data.divergence_pp !== null && (
            <div className="flex items-center justify-between text-xs">
              <span className="text-muted-foreground">Divergence</span>
              <span
                className={cn(
                  "font-medium",
                  data.has_significant_divergence
                    ? "text-red-500"
                    : "text-muted-foreground",
                )}
              >
                {data.divergence_pp.toFixed(1)}pp
              </span>
            </div>
          )}
        </div>

        {/* Stats */}
        <div className="grid grid-cols-2 gap-3 pt-2 border-t">
          <div>
            <p className="text-xs text-muted-foreground">Copy Trades</p>
            <p className="font-medium tabular-nums">{data.copy_trade_count}</p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Copy PnL</p>
            <div className="flex items-center gap-1">
              {pnlPositive ? (
                <TrendingUp className="h-3 w-3 text-profit" />
              ) : (
                <TrendingDown className="h-3 w-3 text-loss" />
              )}
              <p
                className={cn(
                  "font-medium tabular-nums",
                  pnlPositive ? "text-profit" : "text-loss",
                )}
              >
                {formatCurrency(data.copy_total_pnl, { showSign: true })}
              </p>
            </div>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
