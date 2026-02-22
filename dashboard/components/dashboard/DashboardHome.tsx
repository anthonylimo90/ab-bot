"use client";

import { useMemo } from "react";
import Link from "next/link";
import {
  useOpenPositions,
  usePositionsQuery,
} from "@/hooks/queries/usePositionsQuery";
import { useActivity } from "@/hooks/useActivity";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { useAllocationsQuery } from "@/hooks/queries/useAllocationsQuery";
import { useRiskStatusQuery } from "@/hooks/queries/useRiskQuery";
import { MetricCard } from "@/components/shared/MetricCard";
import { ConnectionStatus } from "@/components/shared/ConnectionStatus";
import { LiveIndicator } from "@/components/shared/LiveIndicator";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { PortfolioChart } from "@/components/charts/PortfolioChart";
import { AllocationPie } from "@/components/charts/AllocationPie";
import { LiveActivityFeed } from "@/components/discover/LiveActivityFeed";
import {
  Activity,
  ArrowRight,
  Copy,
  TrendingDown,
  Zap,
  AlertCircle,
  XCircle,
  DollarSign,
  CheckCircle2,
  ShieldAlert,
  Search,
  PieChart,
  TrendingUp,
  Target,
  Settings2,
  Star,
  LineChart,
} from "lucide-react";
import { formatCurrency, formatSkipReason } from "@/lib/utils";
import { cn } from "@/lib/utils";
import { TimeAgo } from "@/components/shared/TimeAgo";

const activityIcons: Record<string, React.ReactNode> = {
  TRADE_COPIED: <Copy className="h-4 w-4 text-blue-500" />,
  TRADE_COPY_SKIPPED: <AlertCircle className="h-4 w-4 text-yellow-500" />,
  TRADE_COPY_FAILED: <XCircle className="h-4 w-4 text-red-500" />,
  STOP_LOSS_TRIGGERED: <TrendingDown className="h-4 w-4 text-loss" />,
  RECOMMENDATION_NEW: <Activity className="h-4 w-4 text-purple-500" />,
  ARBITRAGE_DETECTED: <Zap className="h-4 w-4 text-yellow-500" />,
  ARB_POSITION_OPENED: <DollarSign className="h-4 w-4 text-profit" />,
  ARB_POSITION_CLOSED: <CheckCircle2 className="h-4 w-4 text-blue-500" />,
  ARB_EXECUTION_FAILED: <XCircle className="h-4 w-4 text-red-500" />,
  ARB_EXIT_FAILED: <ShieldAlert className="h-4 w-4 text-red-400" />,
  POSITION_OPENED: <AlertCircle className="h-4 w-4 text-profit" />,
  POSITION_CLOSED: <AlertCircle className="h-4 w-4 text-muted-foreground" />,
  WALLET_DEMOTED: <TrendingDown className="h-4 w-4 text-orange-500" />,
  WALLET_PROMOTED: <TrendingUp className="h-4 w-4 text-profit" />,
};

export function DashboardHome() {
  const { currentWorkspace } = useWorkspaceStore();
  const { data: allocations = [] } = useAllocationsQuery(currentWorkspace?.id);
  const activeWallets = allocations.filter((a) => a.tier === "active");
  const { openPositions, totalUnrealizedPnl } = useOpenPositions();
  const { data: riskStatus } = useRiskStatusQuery(currentWorkspace?.id);
  const { data: closedPositions = [] } = usePositionsQuery({ status: "closed" });
  const stats = useMemo(() => {
    const totalValue = openPositions.reduce(
      (sum, p) => sum + p.quantity * p.current_price,
      0,
    );
    const closedCount = closedPositions.length;
    const winCount = closedPositions.filter(
      (p) => (p.realized_pnl ?? 0) > 0,
    ).length;
    return {
      total_value: totalValue,
      total_pnl_percent: totalValue > 0 ? (totalUnrealizedPnl / totalValue) * 100 : 0,
      today_pnl: riskStatus?.circuit_breaker?.daily_pnl ?? 0,
      today_pnl_percent:
        totalValue > 0
          ? ((riskStatus?.circuit_breaker?.daily_pnl ?? 0) / totalValue) * 100
          : 0,
      active_positions: openPositions.length,
      win_rate: closedCount > 0 ? (winCount / closedCount) * 100 : 0,
    };
  }, [openPositions, totalUnrealizedPnl, closedPositions, riskStatus]);
  const { activities, status: activityStatus, unreadCount } = useActivity();

  // Watchdog: detect zero fills with skips
  const watchdog = useMemo(() => {
    const copiedCount = activities.filter((a) => a.type === "TRADE_COPIED").length;
    const skippedItems = activities.filter((a) => a.type === "TRADE_COPY_SKIPPED");
    if (copiedCount === 0 && skippedItems.length > 0) {
      // Find top skip reason
      const counts: Record<string, number> = {};
      skippedItems.forEach((a) => {
        if (a.skip_reason) counts[a.skip_reason] = (counts[a.skip_reason] || 0) + 1;
      });
      const sorted = Object.entries(counts).sort((a, b) => b[1] - a[1]);
      return {
        show: true,
        skippedCount: skippedItems.length,
        topReason: sorted[0] ? formatSkipReason(sorted[0][0]) : undefined,
      };
    }
    return { show: false, skippedCount: 0 };
  }, [activities]);

  // Derive equity curve from closed positions (accumulated realized P&L)
  const equityCurve = useMemo(() => {
    if (closedPositions.length === 0) return [];
    const sorted = [...closedPositions]
      .filter((p) => p.updated_at)
      .sort(
        (a, b) =>
          new Date(a.updated_at!).getTime() - new Date(b.updated_at!).getTime(),
      );
    let cumulative = 0;
    return sorted.map((p) => {
      cumulative += p.realized_pnl ?? 0;
      return {
        time: new Date(p.updated_at!).toISOString().slice(0, 10),
        value: cumulative,
      };
    });
  }, [closedPositions]);

  // Derive allocation pie data from active wallets
  const allocationData = useMemo(() => {
    const colors = [
      "#3b82f6",
      "#10b981",
      "#f59e0b",
      "#ef4444",
      "#8b5cf6",
    ];
    return activeWallets.map((w, i) => ({
      name: w.wallet_label || `${w.wallet_address.slice(0, 6)}...${w.wallet_address.slice(-4)}`,
      value: Number(w.allocation_pct) || 0,
      color: colors[i % colors.length],
    }));
  }, [activeWallets]);

  const isAutomatic = currentWorkspace?.setup_mode === "automatic";
  const modeLabel = isAutomatic ? "Guided" : "Custom";
  const ModeIcon = isAutomatic ? Target : Settings2;

  return (
    <div className="space-y-5 sm:space-y-6">
      {/* Page Header */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:gap-4">
          <div>
            <h1 className="text-2xl font-bold tracking-tight sm:text-3xl">
              Dashboard
            </h1>
            <p className="text-muted-foreground">
              Monitor your portfolio and trading activity
            </p>
          </div>
          <LiveIndicator />
        </div>
        <Badge variant="secondary" className="w-fit items-center gap-1">
          <ModeIcon className="h-3 w-3" />
          {modeLabel} Mode
        </Badge>
      </div>

      {/* Watchdog alert */}
      {watchdog.show && (
        <div className="flex items-center gap-3 rounded-lg border border-amber-500/30 bg-amber-500/5 px-4 py-3">
          <AlertCircle className="h-5 w-5 text-amber-500 shrink-0" />
          <div className="flex-1 text-sm">
            <span className="font-medium text-amber-600">No trades executing</span>
            {" \u2014 "}
            <span className="text-muted-foreground">
              {watchdog.skippedCount} skipped{watchdog.topReason ? `. Top reason: ${watchdog.topReason}` : ""}
            </span>
          </div>
          <Link href="/history">
            <Button variant="ghost" size="sm" className="text-amber-600 hover:text-amber-700">
              View Details <ArrowRight className="ml-1 h-3 w-3" />
            </Button>
          </Link>
        </div>
      )}

      {/* Stats Grid */}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <MetricCard
          title="Portfolio Value"
          value={formatCurrency(stats.total_value)}
          change={stats.total_pnl_percent}
          trend={stats.total_pnl_percent >= 0 ? "up" : "down"}
        />
        <MetricCard
          title="Today's P&L"
          value={formatCurrency(stats.today_pnl, { showSign: true })}
          change={stats.today_pnl_percent}
          trend={stats.today_pnl >= 0 ? "up" : "down"}
        />
        <MetricCard
          title="Active Wallets"
          value={`${activeWallets.length}/5`}
          changeLabel={
            activeWallets.length < 5
              ? `${5 - activeWallets.length} slots available`
              : "Roster full"
          }
          trend="neutral"
        />
        <MetricCard
          title="Open Positions"
          value={stats.active_positions.toString()}
          changeLabel={`Win rate: ${(stats.win_rate || 0).toFixed(0)}%`}
          trend="neutral"
        />
      </div>

      {/* Quick Actions Row */}
      <div className="flex flex-wrap gap-2">
        <Link href="/discover">
          <Button variant="outline" size="sm" className="gap-2">
            <Search className="h-4 w-4 text-blue-500" />
            Discover
          </Button>
        </Link>
        <Link href="/trading">
          <Button variant="outline" size="sm" className="gap-2">
            <PieChart className="h-4 w-4 text-green-500" />
            Positions ({stats.active_positions})
          </Button>
        </Link>
        <Link href="/backtest">
          <Button variant="outline" size="sm" className="gap-2">
            <LineChart className="h-4 w-4 text-purple-500" />
            Backtest
          </Button>
        </Link>
        <Link href="/roster">
          <Button variant="outline" size="sm" className="gap-2">
            <Star className="h-4 w-4 text-yellow-500" />
            Wallets ({activeWallets.length}/5)
          </Button>
        </Link>
      </div>

      {/* Main Content */}
      <div className="grid gap-4 sm:gap-6 lg:grid-cols-2">
        {/* Recent Activity */}
        <Card>
          <CardHeader className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div className="flex flex-wrap items-center gap-2">
              <CardTitle>Recent Activity</CardTitle>
              {unreadCount > 0 && (
                <span className="flex h-5 min-w-5 items-center justify-center rounded-full bg-primary px-1.5 text-xs font-medium text-primary-foreground">
                  {unreadCount}
                </span>
              )}
              <ConnectionStatus status={activityStatus} />
            </div>
            <Link href="/history" className="w-full sm:w-auto">
              <Button variant="ghost" size="sm" className="w-full sm:w-auto">
                View All
                <ArrowRight className="ml-2 h-4 w-4" />
              </Button>
            </Link>
          </CardHeader>
          <CardContent>
            <div className="max-h-[320px] space-y-4 overflow-y-auto sm:max-h-[350px]">
              {activities.length === 0 ? (
                <p className="text-sm text-muted-foreground text-center py-8">
                  No recent activity yet. Activity will appear here when wallets
                  make trades.
                </p>
              ) : (
                activities.slice(0, 8).map((item, index) => (
                  <div
                    key={item.id}
                    className={cn(
                      "flex flex-wrap items-start gap-3 sm:flex-nowrap",
                      index === 0 && "animate-slide-in",
                    )}
                  >
                    <div className="mt-1">
                      {activityIcons[item.type] || (
                        <Activity className="h-4 w-4" />
                      )}
                    </div>
                    <div className="min-w-0 flex-1 space-y-1">
                      <p className="text-sm break-words">{item.message}</p>
                      <TimeAgo
                        date={item.created_at}
                        className="text-xs text-muted-foreground"
                      />
                    </div>
                    {item.pnl !== undefined && (
                      <span
                        className={cn(
                          "ml-auto text-sm font-medium tabular-nums sm:ml-0",
                          item.pnl >= 0 ? "text-profit" : "text-loss",
                        )}
                      >
                        {item.pnl >= 0 ? "+" : ""}
                        {formatCurrency(item.pnl)}
                      </span>
                    )}
                  </div>
                ))
              )}
            </div>
          </CardContent>
        </Card>

        {/* Live Activity Feed */}
        <LiveActivityFeed className="lg:col-span-1" />
      </div>

      {/* Charts */}
      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>Equity Curve</CardTitle>
          </CardHeader>
          <CardContent>
            {equityCurve.length > 1 ? (
              <PortfolioChart data={equityCurve} height={250} />
            ) : (
              <div className="flex items-center justify-center h-[250px] text-sm text-muted-foreground">
                Close positions to see your equity curve
              </div>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>Wallet Allocation</CardTitle>
          </CardHeader>
          <CardContent>
            {allocationData.length > 0 ? (
              <AllocationPie data={allocationData} showLegend />
            ) : (
              <div className="flex items-center justify-center h-[250px] text-sm text-muted-foreground">
                Add active wallets to see allocation
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
