"use client";

import { useState, useEffect } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ConnectionStatus } from "@/components/shared/ConnectionStatus";
import { PageIntro } from "@/components/shared/PageIntro";
import { useActivity } from "@/hooks/useActivity";
import { formatTimeAgo, cn } from "@/lib/utils";
import {
  Zap,
  TrendingDown,
  Activity,
  XCircle,
  DollarSign,
  CheckCircle2,
  ShieldAlert,
} from "lucide-react";
import type { ActivityType } from "@/types/api";

const activityIcons: Record<string, React.ReactNode> = {
  POSITION_OPENED: <DollarSign className="h-4 w-4 text-profit" />,
  POSITION_CLOSED: <CheckCircle2 className="h-4 w-4 text-blue-500" />,
  STOP_LOSS_TRIGGERED: <TrendingDown className="h-4 w-4 text-loss" />,
  TAKE_PROFIT_TRIGGERED: <CheckCircle2 className="h-4 w-4 text-profit" />,
  ARBITRAGE_DETECTED: <Zap className="h-4 w-4 text-yellow-500" />,
  ARB_POSITION_OPENED: <DollarSign className="h-4 w-4 text-profit" />,
  ARB_POSITION_CLOSED: <CheckCircle2 className="h-4 w-4 text-blue-500" />,
  ARB_EXECUTION_FAILED: <XCircle className="h-4 w-4 text-red-500" />,
  ARB_EXIT_FAILED: <ShieldAlert className="h-4 w-4 text-red-400" />,
};

const ACTIVITY_LABELS: Partial<Record<ActivityType, string>> = {
  POSITION_OPENED: "Position Opened",
  POSITION_CLOSED: "Position Closed",
  STOP_LOSS_TRIGGERED: "Stop Loss Triggered",
  TAKE_PROFIT_TRIGGERED: "Take Profit Triggered",
  ARBITRAGE_DETECTED: "Arbitrage Found",
  ARB_POSITION_OPENED: "Arbitrage Opened",
  ARB_POSITION_CLOSED: "Arbitrage Closed",
  ARB_EXECUTION_FAILED: "Trade Failed",
  ARB_EXIT_FAILED: "Exit Failed",
};

type ActivityFilter = "all" | "Arbitrage" | "StopLoss";

const FILTER_MAP: Record<ActivityFilter, ActivityType[] | null> = {
  all: null,
  Arbitrage: [
    "ARBITRAGE_DETECTED",
    "ARB_POSITION_OPENED",
    "ARB_POSITION_CLOSED",
    "ARB_EXECUTION_FAILED",
    "ARB_EXIT_FAILED",
  ],
  StopLoss: ["STOP_LOSS_TRIGGERED", "TAKE_PROFIT_TRIGGERED"],
};

export default function ActivityPage() {
  const { activities, status, unreadCount, markAsRead } = useActivity();
  const [filter, setFilter] = useState<ActivityFilter>("all");

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
            Activity
          </h1>
          <p className="text-muted-foreground">
            Live trading signals and system events
          </p>
        </div>
        <ConnectionStatus status={status} />
      </div>

      <PageIntro
        title="What shows up here"
        description="This feed records the major events the system sees while it is scanning, trading, pausing, or exiting positions."
        bullets={[
          "Use this page to understand what the system just did in time order.",
          "Arbitrage events are trade opportunities and executions.",
          "Stop loss and take profit events show when the system exited to protect gains or limit losses."
        ]}
      />

      <Tabs
        value={filter}
        onValueChange={(v) => setFilter(v as ActivityFilter)}
      >
        <div className="overflow-x-auto pb-1">
          <TabsList className="w-max min-w-full sm:w-auto">
            <TabsTrigger value="all">All</TabsTrigger>
            <TabsTrigger value="Arbitrage">Arbitrage</TabsTrigger>
            <TabsTrigger value="StopLoss">Stop Loss</TabsTrigger>
          </TabsList>
        </div>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <span>Activity Feed</span>
            <Badge variant="secondary">{filtered.length}</Badge>
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="space-y-3">
            {filtered.length === 0 ? (
              <p className="text-sm text-muted-foreground text-center py-8">
                No activity yet. Events will appear here as trades are detected.
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
                        {ACTIVITY_LABELS[item.type] ?? item.type.replace(/_/g, " ")}
                      </Badge>
                      <span className="text-xs text-muted-foreground">
                        {formatTimeAgo(item.created_at)}
                      </span>
                    </div>
                    <p className="text-sm break-words">{item.message}</p>
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
