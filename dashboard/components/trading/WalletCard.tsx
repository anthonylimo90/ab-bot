"use client";

import { useState, memo } from "react";
import Link from "next/link";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  shortenAddress,
  formatCurrency,
  formatLargePercent,
  cn,
} from "@/lib/utils";
import {
  ChevronDown,
  ChevronUp,
  ArrowRight,
  TrendingUp,
  TrendingDown,
  X,
  Trash2,
  Pin,
  PinOff,
  Clock,
  Zap,
  AlertTriangle,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { differenceInDays } from "date-fns";
import type { CopySettings } from "@/types/api";

interface Position {
  id: string;
  marketId: string;
  marketQuestion?: string;
  outcome: "yes" | "no";
  quantity: number;
  entryPrice: number;
  currentPrice: number;
  pnl: number;
  pnlPercent: number;
}

interface WalletCardProps {
  wallet: {
    address: string;
    label?: string;
    copySettings: CopySettings;
    roi30d: number;
    sharpe: number;
    winRate: number;
    trades: number;
    maxDrawdown: number;
    confidence: number;
    pinned?: boolean;
    probationUntil?: string;
    isAutoSelected?: boolean;
    consecutiveLosses?: number;
  };
  positions: Position[];
  onDemote?: (address: string) => void;
  onPromote?: (address: string) => void;
  onRemove?: (address: string) => void;
  onClosePosition?: (id: string) => void;
  onPin?: (address: string) => void;
  onUnpin?: (address: string) => void;
  isActive?: boolean;
  isRosterFull?: boolean;
  isLoading?: boolean;
  pinsRemaining?: number;
  maxPins?: number;
}

export const WalletCard = memo(function WalletCard({
  wallet,
  positions,
  onDemote,
  onPromote,
  onRemove,
  onClosePosition,
  onPin,
  onUnpin,
  isActive = false,
  isRosterFull = false,
  isLoading = false,
  pinsRemaining = 3,
  maxPins = 3,
}: WalletCardProps) {
  const [isExpanded, setIsExpanded] = useState(false);

  const totalPnl = positions.reduce((sum, p) => sum + p.pnl, 0);
  const hasPositions = positions.length > 0;

  // Automation computed values
  const isInProbation =
    wallet.probationUntil && new Date(wallet.probationUntil) > new Date();
  const probationDaysRemaining = wallet.probationUntil
    ? Math.max(0, differenceInDays(new Date(wallet.probationUntil), new Date()))
    : 0;
  const hasConsecutiveLosses = (wallet.consecutiveLosses || 0) >= 3;

  const copyBehaviorLabels: Record<string, string> = {
    copy_all: "All Trades",
    events_only: "Events Only",
    arb_threshold: "Arb Threshold",
  };

  return (
    <Card className="hover:border-primary/50 transition-colors">
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3 min-w-0">
            <div
              className={cn(
                "h-10 w-10 rounded-full flex items-center justify-center font-bold text-primary-foreground",
                isActive ? "bg-primary" : "bg-muted text-muted-foreground",
              )}
            >
              {wallet.label?.charAt(0) ||
                wallet.address.charAt(2).toUpperCase()}
            </div>
            <div className="min-w-0">
              <div className="flex items-center gap-2 min-w-0">
                <p className="font-medium truncate">
                  {wallet.label || shortenAddress(wallet.address)}
                </p>
                {/* Pinned indicator */}
                {wallet.pinned && (
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger>
                        <Pin className="h-3.5 w-3.5 text-purple-500 fill-purple-500" />
                      </TooltipTrigger>
                      <TooltipContent>
                        <p>Pinned - protected from auto-demotion</p>
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                )}
                {/* Probation badge */}
                {isActive && isInProbation && (
                  <Badge
                    variant="outline"
                    className="text-xs bg-blue-500/10 text-blue-500 border-blue-500/20"
                  >
                    <Clock className="h-3 w-3 mr-1" />
                    {probationDaysRemaining}d left
                  </Badge>
                )}
                {/* Auto-selected indicator */}
                {wallet.isAutoSelected && !isInProbation && (
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger>
                        <Zap className="h-3.5 w-3.5 text-yellow-500" />
                      </TooltipTrigger>
                      <TooltipContent>
                        <p>Auto-selected by automation</p>
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                )}
                {/* Consecutive losses warning */}
                {hasConsecutiveLosses && (
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger>
                        <AlertTriangle className="h-3.5 w-3.5 text-orange-500" />
                      </TooltipTrigger>
                      <TooltipContent>
                        <p>{wallet.consecutiveLosses} consecutive losses</p>
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                )}
              </div>
              <p className="text-xs text-muted-foreground font-mono">
                {shortenAddress(wallet.address)}
              </p>
            </div>
          </div>
          <div className="flex items-center gap-2">
            {isActive && wallet.copySettings && (
              <span className="text-xs px-2 py-1 rounded-full bg-primary/10 text-primary">
                {copyBehaviorLabels[wallet.copySettings.copy_behavior] ||
                  "All Trades"}
              </span>
            )}
          </div>
        </div>
      </CardHeader>

      <CardContent className="space-y-4">
        {/* Metrics Row */}
        <div className="grid grid-cols-5 gap-3 text-sm">
          <div>
            <p className="text-xs text-muted-foreground">ROI (30d)</p>
            <p
              className={cn(
                "font-medium tabular-nums",
                wallet.roi30d >= 0 ? "text-profit" : "text-loss",
              )}
            >
              {formatLargePercent(wallet.roi30d, { showSign: true })}
            </p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Sharpe</p>
            <p className="font-medium tabular-nums">
              {Number(wallet.sharpe).toFixed(2)}
            </p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Win Rate</p>
            <p className="font-medium tabular-nums">
              {Number(wallet.winRate).toFixed(1)}%
            </p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Confidence</p>
            <p
              className={cn(
                "font-medium tabular-nums",
                wallet.confidence >= 80
                  ? "text-profit"
                  : wallet.confidence >= 60
                    ? "text-yellow-500"
                    : "text-muted-foreground",
              )}
            >
              {Number(wallet.confidence).toFixed(0)}%
            </p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">
              {isActive ? "Allocation" : "Max DD"}
            </p>
            <p
              className={cn(
                "font-medium tabular-nums",
                !isActive && "text-loss",
              )}
            >
              {isActive
                ? `${wallet.copySettings?.allocation_pct || 0}%`
                : `${Number(wallet.maxDrawdown).toFixed(1)}%`}
            </p>
          </div>
        </div>

        {/* Allocation Bar (Active only) */}
        {isActive && wallet.copySettings && (
          <div className="space-y-1">
            <div className="w-full bg-muted rounded-full h-2">
              <div
                className="bg-primary h-2 rounded-full transition-all"
                style={{ width: `${wallet.copySettings.allocation_pct}%` }}
              />
            </div>
            <p className="text-xs text-muted-foreground text-center">
              {wallet.copySettings.allocation_pct}% allocation
            </p>
          </div>
        )}

        {/* Actions Row */}
        <div className="flex items-center justify-between pt-2 border-t">
          <div className="flex items-center gap-2">
            <Link href={`/wallet/${wallet.address}`}>
              <Button variant="outline" size="sm">
                View Details
                <ArrowRight className="ml-2 h-4 w-4" />
              </Button>
            </Link>
            {isActive ? (
              <>
                {/* Pin/Unpin button */}
                {wallet.pinned ? (
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => onUnpin?.(wallet.address)}
                          disabled={isLoading}
                          className="text-purple-500 hover:text-purple-600"
                        >
                          <PinOff className="mr-1 h-4 w-4" />
                          Unpin
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent>
                        <p>Remove protection from auto-demotion</p>
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                ) : (
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => onPin?.(wallet.address)}
                          disabled={isLoading || pinsRemaining <= 0}
                        >
                          <Pin className="mr-1 h-4 w-4" />
                          Pin
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent>
                        {pinsRemaining > 0 ? (
                          <p>
                            Pin to protect from auto-demotion ({pinsRemaining}{" "}
                            of {maxPins} remaining)
                          </p>
                        ) : (
                          <p>No pins remaining (max {maxPins})</p>
                        )}
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                )}
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => onDemote?.(wallet.address)}
                  disabled={isLoading}
                >
                  <ChevronDown className="mr-1 h-4 w-4" />
                  Demote
                </Button>
              </>
            ) : (
              <>
                <Button
                  variant="default"
                  size="sm"
                  onClick={() => onPromote?.(wallet.address)}
                  disabled={isLoading || isRosterFull}
                >
                  <ChevronUp className="mr-1 h-4 w-4" />
                  Promote
                </Button>
                {onRemove && (
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={() => onRemove(wallet.address)}
                    disabled={isLoading}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                )}
              </>
            )}
          </div>

          {/* Expand/Collapse for positions */}
          {isActive && hasPositions && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setIsExpanded(!isExpanded)}
            >
              {isExpanded ? (
                <>
                  <ChevronUp className="mr-1 h-4 w-4" />
                  Hide Positions
                </>
              ) : (
                <>
                  <ChevronDown className="mr-1 h-4 w-4" />
                  {positions.length} Position{positions.length !== 1 ? "s" : ""}
                </>
              )}
            </Button>
          )}
        </div>

        {/* Positions (Expanded) */}
        {isActive && isExpanded && hasPositions && (
          <div className="space-y-2 pt-2 border-t">
            <p className="text-xs font-medium text-muted-foreground uppercase tracking-wider">
              Positions from this wallet
            </p>
            <div className="space-y-2">
              {positions.map((position) => (
                <div
                  key={position.id}
                  className="flex items-center justify-between p-3 rounded-lg bg-muted/30 hover:bg-muted/50 transition-colors"
                >
                  <div className="flex items-center gap-3">
                    <span
                      className={cn(
                        "px-2 py-0.5 rounded text-xs font-medium uppercase",
                        position.outcome === "yes"
                          ? "bg-profit/10 text-profit"
                          : "bg-loss/10 text-loss",
                      )}
                    >
                      {position.outcome}
                    </span>
                    <div>
                      <p className="text-sm font-medium">
                        {position.marketQuestion ||
                          position.marketId.slice(0, 30) + "..."}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {position.quantity} @ ${position.entryPrice.toFixed(2)}
                      </p>
                    </div>
                  </div>
                  <div className="flex items-center gap-3">
                    <div className="text-right">
                      <div className="flex items-center justify-end gap-1">
                        {position.pnl >= 0 ? (
                          <TrendingUp className="h-3 w-3 text-profit" />
                        ) : (
                          <TrendingDown className="h-3 w-3 text-loss" />
                        )}
                        <span
                          className={cn(
                            "text-sm font-medium tabular-nums",
                            position.pnl >= 0 ? "text-profit" : "text-loss",
                          )}
                        >
                          {formatCurrency(position.pnl, { showSign: true })}
                        </span>
                      </div>
                      <p className="text-xs text-muted-foreground tabular-nums">
                        {position.pnlPercent >= 0 ? "+" : ""}
                        {position.pnlPercent.toFixed(1)}%
                      </p>
                    </div>
                    {onClosePosition && (
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7"
                        onClick={() => onClosePosition(position.id)}
                      >
                        <X className="h-3 w-3" />
                      </Button>
                    )}
                  </div>
                </div>
              ))}

              {/* Total P&L for this wallet */}
              <div className="flex items-center justify-between p-2 border-t">
                <span className="text-sm text-muted-foreground">
                  Wallet P&L
                </span>
                <span
                  className={cn(
                    "text-sm font-medium",
                    totalPnl >= 0 ? "text-profit" : "text-loss",
                  )}
                >
                  {formatCurrency(totalPnl, { showSign: true })}
                </span>
              </div>
            </div>
          </div>
        )}

        {/* No Positions Message */}
        {isActive && isExpanded && !hasPositions && (
          <div className="pt-2 border-t text-center py-4">
            <p className="text-sm text-muted-foreground">
              No positions from this wallet yet
            </p>
          </div>
        )}
      </CardContent>
    </Card>
  );
});
