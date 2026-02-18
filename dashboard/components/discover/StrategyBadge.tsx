"use client";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

const strategyConfig: Record<
  string,
  { label: string; color: string; description: string }
> = {
  Momentum: {
    label: "Momentum",
    color: "text-blue-600 bg-blue-500/10 border-blue-500/20",
    description: "Follows trends and market momentum",
  },
  Arbitrage: {
    label: "Arbitrage",
    color: "text-purple-600 bg-purple-500/10 border-purple-500/20",
    description: "Exploits price differences across markets",
  },
  EventTrader: {
    label: "Event",
    color: "text-amber-600 bg-amber-500/10 border-amber-500/20",
    description: "Trades around specific events and catalysts",
  },
  MarketMaker: {
    label: "Market Maker",
    color: "text-cyan-600 bg-cyan-500/10 border-cyan-500/20",
    description: "Provides liquidity by quoting both sides",
  },
  Contrarian: {
    label: "Contrarian",
    color: "text-rose-600 bg-rose-500/10 border-rose-500/20",
    description: "Bets against consensus positions",
  },
  Scalper: {
    label: "Scalper",
    color: "text-teal-600 bg-teal-500/10 border-teal-500/20",
    description: "Makes frequent small-profit trades",
  },
  Unknown: {
    label: "Mixed",
    color: "text-muted-foreground bg-muted border-muted-foreground/20",
    description: "No dominant strategy pattern detected",
  },
};

interface StrategyBadgeProps {
  strategy?: string | null;
  size?: "sm" | "md";
}

export function StrategyBadge({ strategy, size = "sm" }: StrategyBadgeProps) {
  if (!strategy) return null;

  const config = strategyConfig[strategy] || strategyConfig.Unknown;
  const sizeClasses = size === "sm" ? "text-[10px] px-1.5 py-0.5" : "text-xs px-2 py-0.5";

  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            className={`inline-flex items-center rounded-full border font-medium ${config.color} ${sizeClasses}`}
          >
            {config.label}
          </span>
        </TooltipTrigger>
        <TooltipContent side="top">
          <p className="text-sm">{config.description}</p>
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
