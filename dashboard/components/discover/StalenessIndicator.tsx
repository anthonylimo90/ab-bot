"use client";

import { Clock } from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

interface StalenessIndicatorProps {
  stalenessDays: number;
  /** Threshold in days above which warning is shown (default 14) */
  warnThreshold?: number;
  /** Threshold in days above which danger styling is shown (default 30) */
  dangerThreshold?: number;
  /** Show even when fresh (default false) */
  showWhenFresh?: boolean;
}

export function StalenessIndicator({
  stalenessDays,
  warnThreshold = 14,
  dangerThreshold = 30,
  showWhenFresh = false,
}: StalenessIndicatorProps) {
  if (!showWhenFresh && stalenessDays < warnThreshold) {
    return null;
  }

  const isDanger = stalenessDays >= dangerThreshold;
  const isWarn = stalenessDays >= warnThreshold;

  const colorClass = isDanger
    ? "text-red-500"
    : isWarn
      ? "text-orange-500"
      : "text-muted-foreground";

  const label =
    stalenessDays === 0
      ? "Active today"
      : stalenessDays === 1
        ? "1 day ago"
        : `${stalenessDays}d ago`;

  const description = isDanger
    ? "This wallet has been inactive for over a month. Data may be unreliable."
    : isWarn
      ? "This wallet has been inactive for over 2 weeks. Score is penalized."
      : "Last trade activity";

  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            className={`inline-flex items-center gap-1 text-[10px] font-medium ${colorClass}`}
          >
            <Clock className="h-3 w-3" />
            {label}
          </span>
        </TooltipTrigger>
        <TooltipContent side="top">
          <p className="text-sm">{description}</p>
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
