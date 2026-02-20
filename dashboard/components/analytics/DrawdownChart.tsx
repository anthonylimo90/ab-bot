"use client";

import { Progress } from "@/components/ui/progress";
import { cn, formatCurrency, formatPercent } from "@/lib/utils";

interface DrawdownChartProps {
  peakValue: number;
  currentValue: number;
  dailyPnl?: number;
  className?: string;
}

export function DrawdownChart({
  peakValue,
  currentValue,
  dailyPnl,
  className,
}: DrawdownChartProps) {
  const drawdownPct = peakValue > 0 ? ((peakValue - currentValue) / peakValue) * 100 : 0;
  const maxThreshold = 20; // 20% max drawdown threshold
  const proximity = Math.min((drawdownPct / maxThreshold) * 100, 100);

  return (
    <div className={cn("space-y-2", className)}>
      <div className="flex items-center justify-between text-sm">
        <span className="text-muted-foreground">Drawdown</span>
        <span
          className={cn(
            "font-medium tabular-nums",
            drawdownPct > 15 ? "text-loss" : drawdownPct > 10 ? "text-yellow-500" : "text-profit",
          )}
        >
          {formatPercent(drawdownPct)}
        </span>
      </div>
      <Progress
        value={proximity}
        className={cn(
          "h-2",
          proximity > 75 ? "[&>div]:bg-loss" : proximity > 50 ? "[&>div]:bg-yellow-500" : "[&>div]:bg-profit",
        )}
      />
      <div className="flex justify-between text-xs text-muted-foreground">
        <span>Peak: {formatCurrency(peakValue)}</span>
        <span>Current: {formatCurrency(currentValue)}</span>
      </div>
      {dailyPnl !== undefined && (
        <div className="text-xs text-muted-foreground">
          Today: <span className={cn("font-medium", dailyPnl >= 0 ? "text-profit" : "text-loss")}>
            {dailyPnl >= 0 ? "+" : ""}{formatCurrency(dailyPnl)}
          </span>
        </div>
      )}
    </div>
  );
}
