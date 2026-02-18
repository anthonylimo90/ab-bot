"use client";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

interface CompositeScoreGaugeProps {
  score?: number | null;
  /** Show as compact inline badge (default) or larger display */
  variant?: "badge" | "display";
}

function getScoreColor(score: number): string {
  if (score >= 75) return "text-profit bg-profit/10 border-profit/20";
  if (score >= 50) return "text-yellow-600 bg-yellow-500/10 border-yellow-500/20";
  if (score >= 25) return "text-orange-600 bg-orange-500/10 border-orange-500/20";
  return "text-loss bg-loss/10 border-loss/20";
}

function getScoreLabel(score: number): string {
  if (score >= 75) return "Excellent";
  if (score >= 50) return "Good";
  if (score >= 25) return "Fair";
  return "Low";
}

export function CompositeScoreGauge({
  score,
  variant = "badge",
}: CompositeScoreGaugeProps) {
  if (score == null) return null;

  const rounded = Math.round(score);
  const color = getScoreColor(rounded);
  const label = getScoreLabel(rounded);

  if (variant === "display") {
    return (
      <div className="flex flex-col items-center gap-1">
        <span className={`text-2xl font-bold tabular-nums ${color.split(" ")[0]}`}>
          {rounded}
        </span>
        <span className="text-[10px] text-muted-foreground uppercase tracking-wider">
          Score
        </span>
      </div>
    );
  }

  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs font-semibold tabular-nums ${color}`}
          >
            {rounded}
          </span>
        </TooltipTrigger>
        <TooltipContent side="top">
          <p className="text-sm">
            Composite Score: {rounded}/100 ({label})
          </p>
          <p className="text-xs text-muted-foreground">
            Multi-factor rating combining ROI, Sharpe, win rate, consistency, and risk
          </p>
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
