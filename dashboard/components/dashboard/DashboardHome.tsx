"use client";

import { useMemo } from "react";
import Link from "next/link";
import {
  usePositionsQuery,
  usePositionsSummaryQuery,
} from "@/hooks/queries/usePositionsQuery";
import { useActivity } from "@/hooks/useActivity";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { useRiskStatusQuery, useDynamicTunerQuery } from "@/hooks/queries/useRiskQuery";
import { InfoTooltip } from "@/components/shared/InfoTooltip";
import { MetricCard } from "@/components/shared/MetricCard";
import { MarketRegimeBadge } from "@/components/shared/MarketRegimeBadge";
import { PageIntro } from "@/components/shared/PageIntro";
import { Badge } from "@/components/ui/badge";
import { ConnectionStatus } from "@/components/shared/ConnectionStatus";
import { LiveIndicator } from "@/components/shared/LiveIndicator";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { PortfolioChart } from "@/components/charts/PortfolioChart";
import {
  Activity,
  ArrowRight,
  TrendingDown,
  Zap,
  DollarSign,
  CheckCircle2,
  ShieldAlert,
  AlertCircle,
  XCircle,
  LineChart,
  BarChart2,
} from "lucide-react";
import { formatCurrency } from "@/lib/utils";
import { cn } from "@/lib/utils";
import { TimeAgo } from "@/components/shared/TimeAgo";

const activityIcons: Record<string, React.ReactNode> = {
  STOP_LOSS_TRIGGERED: <TrendingDown className="h-4 w-4 text-loss" />,
  ARBITRAGE_DETECTED: <Zap className="h-4 w-4 text-yellow-500" />,
  ARB_POSITION_OPENED: <DollarSign className="h-4 w-4 text-profit" />,
  ARB_POSITION_CLOSED: <CheckCircle2 className="h-4 w-4 text-blue-500" />,
  ARB_EXECUTION_FAILED: <XCircle className="h-4 w-4 text-red-500" />,
  ARB_EXIT_FAILED: <ShieldAlert className="h-4 w-4 text-red-400" />,
  POSITION_OPENED: <AlertCircle className="h-4 w-4 text-profit" />,
  POSITION_CLOSED: <AlertCircle className="h-4 w-4 text-muted-foreground" />,
  TRADE_EXECUTED: <CheckCircle2 className="h-4 w-4 text-blue-500" />,
  TRADE_FAILED: <XCircle className="h-4 w-4 text-red-500" />,
  TRADE_PENDING: <Activity className="h-4 w-4 text-muted-foreground" />,
};

const activityLabels: Record<string, string> = {
  STOP_LOSS_TRIGGERED: "Stop Loss Triggered",
  ARBITRAGE_DETECTED: "Arbitrage Found",
  ARB_POSITION_OPENED: "Arbitrage Opened",
  ARB_POSITION_CLOSED: "Arbitrage Closed",
  ARB_EXECUTION_FAILED: "Trade Failed",
  ARB_EXIT_FAILED: "Exit Failed",
  POSITION_OPENED: "Position Opened",
  POSITION_CLOSED: "Position Closed",
  TRADE_EXECUTED: "Trade Executed",
  TRADE_FAILED: "Trade Failed",
  TRADE_PENDING: "Trade Pending",
};

export function DashboardHome() {
  const { currentWorkspace } = useWorkspaceStore();
  const { data: positionsSummary } = usePositionsSummaryQuery();
  const { data: riskStatus } = useRiskStatusQuery(currentWorkspace?.id);
  const { data: dynamicTunerStatus } = useDynamicTunerQuery(currentWorkspace?.id);
  const { data: closedPositions = [] } = usePositionsQuery({
    status: "closed",
    limit: 500,
  });
  const isTradingManuallyPaused =
    Boolean(currentWorkspace) &&
    !currentWorkspace?.live_trading_enabled &&
    !currentWorkspace?.arb_auto_execute;
  const isCircuitBreakerPaused = Boolean(riskStatus?.circuit_breaker?.tripped);
  const automationStatus = isCircuitBreakerPaused
    ? {
        label: "Paused",
        variant: "destructive" as const,
        className: "text-xs",
        tooltip:
          "The circuit breaker is active, so automated trading is paused until the risk controls allow it again.",
      }
    : isTradingManuallyPaused
      ? {
          label: "Manually paused",
          variant: "warning" as const,
          className: "text-xs",
          tooltip:
            "Automation is paused from workspace settings, even though the automatic safety stop is not currently tripped.",
        }
      : {
          label: "Healthy",
          variant: "outline" as const,
          className: "bg-profit/10 text-profit border-profit/20 text-xs",
          tooltip:
            "Automated trading is enabled and the safety stop is not currently blocking trading.",
        };
  const stats = useMemo(() => {
    const totalValue = positionsSummary?.portfolio_value ?? 0;
    const totalUnrealizedPnl = positionsSummary?.unrealized_pnl ?? 0;
    return {
      total_value: totalValue,
      total_pnl_percent: totalValue > 0 ? (totalUnrealizedPnl / totalValue) * 100 : 0,
      total_unrealized_pnl: totalUnrealizedPnl,
      today_pnl: riskStatus?.circuit_breaker?.daily_pnl ?? 0,
      today_pnl_percent:
        totalValue > 0
          ? ((riskStatus?.circuit_breaker?.daily_pnl ?? 0) / totalValue) * 100
          : 0,
      active_positions: positionsSummary?.open_positions ?? 0,
      win_rate: positionsSummary?.win_rate ?? 0,
    };
  }, [positionsSummary, riskStatus]);
  const { activities, status: activityStatus, unreadCount } = useActivity();

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
      </div>

      <PageIntro
        title="What this dashboard shows"
        description="This is your high-level system summary. It tells you whether the bot is healthy, what your open trades look like, and what the system has done most recently."
        bullets={[
          "Start with the status strip to confirm trading is safe and the scanner is active.",
          "Portfolio metrics summarize current exposure and recent profit or loss.",
          "Recent Activity gives the fastest explanation of what the system just did."
        ]}
      />

      {/* System Status Strip */}
      {(riskStatus || dynamicTunerStatus) && (
        <Card>
          <CardContent className="p-4">
            <div className="flex flex-wrap items-center gap-4">
              {/* Circuit Breaker */}
              <div className="flex items-center gap-2">
                <span className="inline-flex items-center gap-1 text-xs text-muted-foreground">
                  Automated Safety
                  <InfoTooltip content={automationStatus.tooltip} />
                </span>
                <Badge
                  variant={automationStatus.variant}
                  className={automationStatus.className}
                >
                  {automationStatus.label}
                </Badge>
              </div>
              {/* Market Regime */}
              <div className="flex items-center gap-2">
                <span className="inline-flex items-center gap-1 text-xs text-muted-foreground">
                  Market Conditions
                  <InfoTooltip content="This is the system's short label for the current market environment, such as calm, volatile, bullish, or bearish." />
                </span>
                <MarketRegimeBadge />
              </div>
              {/* Scanner Markets */}
              {dynamicTunerStatus?.scanner_status && (
                <div className="flex items-center gap-2">
                  <span className="inline-flex items-center gap-1 text-xs text-muted-foreground">
                    Scanner Coverage
                    <InfoTooltip content="This shows how many markets the bot is actively watching right now. Core markets are higher-priority, while explore markets are experimental." />
                  </span>
                  <span className="text-xs font-medium">
                    {dynamicTunerStatus.scanner_status.monitored_markets} markets
                  </span>
                  <span className="text-xs text-muted-foreground">
                    ({dynamicTunerStatus.scanner_status.core_markets} core / {dynamicTunerStatus.scanner_status.exploration_markets} explore)
                  </span>
                </div>
              )}
            </div>
          </CardContent>
        </Card>
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
          title="Unrealized P&L"
          value={formatCurrency(stats.total_unrealized_pnl, { showSign: true })}
          trend={
            stats.total_unrealized_pnl > 0
              ? "up"
              : stats.total_unrealized_pnl < 0
                ? "down"
                : "neutral"
          }
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
        <Link href="/markets">
          <Button variant="outline" size="sm" className="gap-2">
            <BarChart2 className="h-4 w-4 text-blue-500" />
            Markets
          </Button>
        </Link>
        <Link href="/signals">
          <Button variant="outline" size="sm" className="gap-2">
            <Zap className="h-4 w-4 text-green-500" />
            Quant Signals
          </Button>
        </Link>
        <Link href="/backtest">
          <Button variant="outline" size="sm" className="gap-2">
            <LineChart className="h-4 w-4 text-purple-500" />
            Backtest
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
                  No recent activity yet. Activity will appear here when trades
                  occur.
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
                      <p className="text-xs text-muted-foreground">
                        {activityLabels[item.type] ?? item.type}
                      </p>
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

        {/* Equity Curve */}
        <Card>
          <CardHeader>
            <CardTitle>Equity Curve</CardTitle>
          </CardHeader>
          <CardContent>
            {equityCurve.length > 1 ? (
              <PortfolioChart data={equityCurve} height={320} />
            ) : (
              <div className="flex items-center justify-center h-[320px] text-sm text-muted-foreground">
                Close positions to see your equity curve
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
