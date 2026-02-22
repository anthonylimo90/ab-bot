"use client";

import { useState, useEffect } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ConnectionStatus } from "@/components/shared/ConnectionStatus";
import { useActivity } from "@/hooks/useActivity";
import { formatTimeAgo, cn, formatSkipReason } from "@/lib/utils";
import {
  Zap,
  Copy,
  TrendingDown,
  AlertCircle,
  Activity,
  XCircle,
  DollarSign,
  CheckCircle2,
  ShieldAlert,
} from "lucide-react";
import type { ActivityType } from "@/types/api";

const activityIcons: Record<string, React.ReactNode> = {
  TRADE_COPIED: <Copy className="h-4 w-4 text-blue-500" />,
  TRADE_COPY_SKIPPED: <AlertCircle className="h-4 w-4 text-yellow-500" />,
  TRADE_COPY_FAILED: <XCircle className="h-4 w-4 text-red-500" />,
  STOP_LOSS_TRIGGERED: <TrendingDown className="h-4 w-4 text-loss" />,
  TAKE_PROFIT_TRIGGERED: <CheckCircle2 className="h-4 w-4 text-profit" />,
  RECOMMENDATION_NEW: <Activity className="h-4 w-4 text-purple-500" />,
  ARBITRAGE_DETECTED: <Zap className="h-4 w-4 text-yellow-500" />,
  ARB_POSITION_OPENED: <DollarSign className="h-4 w-4 text-profit" />,
  ARB_POSITION_CLOSED: <CheckCircle2 className="h-4 w-4 text-blue-500" />,
  ARB_EXECUTION_FAILED: <XCircle className="h-4 w-4 text-red-500" />,
  ARB_EXIT_FAILED: <ShieldAlert className="h-4 w-4 text-red-400" />,
};

type SignalFilter = "all" | "CopyTrade" | "Arbitrage" | "StopLoss" | "Alert";

const FILTER_MAP: Record<SignalFilter, ActivityType[] | null> = {
  all: null,
  CopyTrade: ["TRADE_COPIED", "TRADE_COPY_SKIPPED", "TRADE_COPY_FAILED"],
  Arbitrage: [
    "ARBITRAGE_DETECTED",
    "ARB_POSITION_OPENED",
    "ARB_POSITION_CLOSED",
    "ARB_EXECUTION_FAILED",
    "ARB_EXIT_FAILED",
  ],
  StopLoss: ["STOP_LOSS_TRIGGERED", "TAKE_PROFIT_TRIGGERED"],
  Alert: ["RECOMMENDATION_NEW"],
};

export default function SignalsPage() {
  const { activities, status, unreadCount, markAsRead } = useActivity();
  const [filter, setFilter] = useState<SignalFilter>("all");

  useEffect(() => {
    markAsRead();
  }, [markAsRead]);

  const filtered = filter === "all"
    ? activities
    : activities.filter((a) => FILTER_MAP[filter]?.includes(a.type));

  return (
    <div className="space-y-5 sm:space-y-6 p-6">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h1 className="flex items-center gap-2 text-2xl font-bold tracking-tight sm:text-3xl">
            <Zap className="h-8 w-8" />
            Signals
          </h1>
          <p className="text-muted-foreground">
            Live trading signals and system events
          </p>
        </div>
        <ConnectionStatus status={status} />
      </div>

      <Tabs
        value={filter}
        onValueChange={(v) => setFilter(v as SignalFilter)}
      >
        <div className="overflow-x-auto pb-1">
          <TabsList className="w-max min-w-full sm:w-auto">
            <TabsTrigger value="all">All</TabsTrigger>
            <TabsTrigger value="CopyTrade">Copy Trade</TabsTrigger>
            <TabsTrigger value="Arbitrage">Arbitrage</TabsTrigger>
            <TabsTrigger value="StopLoss">Stop Loss</TabsTrigger>
            <TabsTrigger value="Alert">Alert</TabsTrigger>
          </TabsList>
        </div>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <span>Signal Feed</span>
            <Badge variant="secondary">{filtered.length}</Badge>
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="space-y-3">
            {filtered.length === 0 ? (
              <p className="text-sm text-muted-foreground text-center py-8">
                No signals yet. Activity will appear here as trades are detected.
              </p>
            ) : (
              filtered.map((item, index) => (
                <div
                  key={item.id}
                  className={cn(
                    "flex items-start gap-3 p-3 rounded-lg border hover:bg-muted/30 transition-colors",
                    index === 0 && "animate-slide-in",
                  )}
                >
                  <div className="mt-0.5">
                    {activityIcons[item.type] || (
                      <Activity className="h-4 w-4" />
                    )}
                  </div>
                  <div className="min-w-0 flex-1 space-y-1">
                    <div className="flex items-center gap-2">
                      <Badge variant="outline" className="text-xs">
                        {item.type.replace(/_/g, " ")}
                      </Badge>
                      <span className="text-xs text-muted-foreground">
                        {formatTimeAgo(item.created_at)}
                      </span>
                    </div>
                    <p className="text-sm break-words">{item.message}</p>
                    {item.type === "TRADE_COPY_SKIPPED" && item.skip_reason && (
                      <Badge
                        variant="outline"
                        className="text-xs bg-yellow-500/10 text-yellow-600 border-yellow-500/20 w-fit"
                      >
                        {formatSkipReason(item.skip_reason)}
                      </Badge>
                    )}
                  </div>
                  {item.pnl !== undefined && (
                    <span
                      className={cn(
                        "text-sm font-medium tabular-nums shrink-0",
                        item.pnl >= 0 ? "text-profit" : "text-loss",
                      )}
                    >
                      {item.pnl >= 0 ? "+" : ""}
                      ${Math.abs(item.pnl).toFixed(2)}
                    </span>
                  )}
                </div>
              ))
            )}
          </div>
        </CardContent>
      </Card>
      </Tabs>
    </div>
  );
}
