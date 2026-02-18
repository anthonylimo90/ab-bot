"use client";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useMarketRegimeQuery } from "@/hooks/queries/useDiscoverQuery";
import type { MarketRegimeType } from "@/types/api";

const regimeStyles: Record<
  MarketRegimeType,
  { bg: string; text: string; border: string }
> = {
  BullVolatile: {
    bg: "bg-green-500/10",
    text: "text-green-600",
    border: "border-green-500/20",
  },
  BullCalm: {
    bg: "bg-emerald-500/10",
    text: "text-emerald-600",
    border: "border-emerald-500/20",
  },
  BearVolatile: {
    bg: "bg-red-500/10",
    text: "text-red-600",
    border: "border-red-500/20",
  },
  BearCalm: {
    bg: "bg-orange-500/10",
    text: "text-orange-600",
    border: "border-orange-500/20",
  },
  Ranging: {
    bg: "bg-blue-500/10",
    text: "text-blue-600",
    border: "border-blue-500/20",
  },
  Uncertain: {
    bg: "bg-muted",
    text: "text-muted-foreground",
    border: "border-muted-foreground/20",
  },
};

export function MarketRegimeBadge() {
  const { data: regime, isLoading } = useMarketRegimeQuery();

  if (isLoading || !regime) {
    return null;
  }

  const style = regimeStyles[regime.regime] || regimeStyles.Uncertain;

  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            className={`inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full text-xs font-medium border ${style.bg} ${style.text} ${style.border}`}
          >
            <span>{regime.icon}</span>
            <span>{regime.label}</span>
          </span>
        </TooltipTrigger>
        <TooltipContent side="bottom" className="max-w-[250px]">
          <p className="text-sm">{regime.description}</p>
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
