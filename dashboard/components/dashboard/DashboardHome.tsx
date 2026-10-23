"use client";

import { useMemo } from "react";
import Link from "next/link";
import {
  usePositionsSummaryQuery,
} from "@/hooks/queries/usePositionsQuery";
import { useAccountHistoryQuery, useAccountSummaryQuery } from "@/hooks/queries/useAccountQuery";
import { useTradeFlowSummaryQuery } from "@/hooks/queries/useTradeFlowQuery";
import { useActivity } from "@/hooks/useActivity";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { useRiskStatusQuery, useDynamicTunerQuery } from "@/hooks/queries/useRiskQuery";
import { useTradingSwitches } from "@/hooks/useTradingSwitches";
import { InfoTooltip } from "@/components/shared/InfoTooltip";
import { MetricCard } from "@/components/shared/MetricCard";
import { MarketRegimeBadge } from "@/components/shared/MarketRegimeBadge";
import { PageIntro } from "@/components/shared/PageIntro";
import { Badge } from "@/components/ui/badge";
import { ConnectionStatus } from "@/components/shared/ConnectionStatus";
import { LiveIndicator, anyStale } from "@/components/shared/LiveIndicator";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { PortfolioChart } from "@/components/charts/PortfolioChart";
import {
  Activity,
  ArrowRight,
  Zap,
  DollarSign,
  CheckCircle2,
  ShieldAlert,
  AlertCircle,
  XCircle,
  LineChart,
  BarChart2,
} from "lucide-react";
import {
  cn,
  formatCurrency,
  formatCurrencyOrDash,
  formatTimeAgo,
  formatWinRatePercent,
} from "@/lib/utils";
import { TimeAgo } from "@/components/shared/TimeAgo";

const activityIcons: Record<string, React.ReactNode> = {
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

const STRATEGY_PNL_LABELS: Record<string, string> = {
  arb: "Arb",
  flow: "Flow",
  mean_reversion: "Mean Rev",
  cross_market: "Cross Mkt",
  resolution_proximity: "Resolution",
};

const activityLabels: Record<string, string> = {
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
  const positionsSummaryQuery = usePositionsSummaryQuery();
  const positionsSummary = positionsSummaryQuery.data;
  const accountSummaryQuery = useAccountSummaryQuery(currentWorkspace?.id);
  const accountSummary = accountSummaryQuery.data;
  const accountHistoryQuery = useAccountHistoryQuery(currentWorkspace?.id, {
    hours: 24,
    limit: 288,
  });
  const accountHistory = accountHistoryQuery.data;
  const {
    data: riskStatus,
    isPending: riskPending,
    isError: riskFailed,
  } = useRiskStatusQuery(currentWorkspace?.id);
  const dynamicTunerQuery = useDynamicTunerQuery(currentWorkspace?.id);
  const dynamicTunerStatus = dynamicTunerQuery.data;
  const tradeFlowQuery = useTradeFlowSummaryQuery({ limit: 100 });
  const tradeFlowSummary = tradeFlowQuery.data;
  // As the server applies the switches: the environment can hold either on.
  // Until its switches are read, "off in the workspace" is not "paused".
  const { liveTrading, arbAutoExecute, environmentKnown, serviceStatus } = useTradingSwitches();
  const switchesOff = Boolean(currentWorkspace) && !liveTrading && !arbAutoExecute;
  const isTradingManuallyPaused = switchesOff && environmentKnown;
  const switchesUnknown = switchesOff && !environmentKnown;
  // Holds the server keeps on new entries whatever the breaker says
  // (settings it cannot read, closes not yet counted).
  const entryHolds = riskStatus?.circuit_breaker?.entry_holds ?? [];
  // New live entries the trading process refuses whatever the switches say
  // (exits it cannot run or that keep failing).
  const liveEntriesHeld = serviceStatus?.live_entries_held ?? null;
  const breaker = riskStatus?.circuit_breaker;
  const isHardKilled = breaker?.tripped === true && breaker.trip_reason === "hard_kill_switch";
  // A switched-off breaker stops its limits, not a latched trip (the hard
  // kill or a manual trip): only those still block.
  const breakerOff = breaker?.config.enabled === false;
  const isCircuitBreakerPaused =
    breaker?.tripped === true &&
    (!breakerOff || breaker.trip_reason === "manual" || isHardKilled);
  // Most severe first. A breaker switched off outranks an unread environment
  // (trading may be live and unguarded), but not a known pause.
  const automationStatus = isHardKilled
    ? {
        label: "HARD KILL",
        variant: "destructive" as const,
        className: "text-xs animate-pulse",
        tooltip:
          "Hard kill switch activated \u2014 trading is permanently halted. Manual reset required via the Risk page.",
      }
    : entryHolds.length > 0
      ? {
          label: "Entries held",
          variant: "destructive" as const,
          className: "text-xs",
          tooltip: `New entries are held whatever the circuit breaker says: ${entryHolds.join("; ")}. Exits still run.`,
        }
    : liveEntriesHeld
      ? {
          label: "Live entries held",
          variant: "destructive" as const,
          className: "text-xs",
          tooltip: `New live entries wait: ${liveEntriesHeld}.`,
        }
    : isCircuitBreakerPaused
      ? {
          label: "Paused",
          variant: "destructive" as const,
          className: "text-xs",
          tooltip:
            "The circuit breaker is active, so automated trading is paused until the risk controls allow it again.",
        }
      : riskPending
      ? {
          label: "Checking",
          variant: "outline" as const,
          className: "text-xs",
          tooltip: "Reading the circuit breaker's status.",
        }
      : !breaker || riskFailed
      ? {
          label: "Risk status unavailable",
          variant: "warning" as const,
          className: "text-xs",
          tooltip:
            "The circuit breaker's status could not be read (or not lately), so whether it is blocking trading is unknown.",
        }
      : breakerOff && !isTradingManuallyPaused
      ? {
          label: "No safety stop",
          variant: "warning" as const,
          className: "text-xs",
          tooltip:
            "The circuit breaker is switched off, so its loss and drawdown limits do not stop trading. Switch it on in Settings.",
        }
      : switchesUnknown
      ? {
          label: "Switches unknown",
          variant: "warning" as const,
          className: "text-xs",
          tooltip:
            "Live trading and arb execution are off in workspace settings, but the server's environment could not be read yet: it may switch them on.",
        }
      : isTradingManuallyPaused
      ? {
          label: "Manually paused",
          variant: "warning" as const,
          className: "text-xs",
          tooltip:
            "Live trading and arb execution are switched off, in workspace settings and in the server's environment, even though the automatic safety stop is not tripped.",
        }
      : {
          label: "Healthy",
          variant: "outline" as const,
          className: "bg-profit/10 text-profit border-profit/20 text-xs",
          tooltip:
            "Automated trading is enabled and the safety stop is not currently blocking trading.",
        };
  const stats = useMemo(() => {
    // Null until the summary loads, and when the wallet's cash could not be
    // read: never shown as $0.
    const totalValue = accountSummary?.total_equity ?? null;
    const totalUnrealizedPnl = accountSummary?.unrealized_pnl ?? null;
    return {
      total_value: totalValue,
      cash_unavailable: accountSummary != null && accountSummary.cash_balance == null,
      // A valuation the drawdown guard does not use: paper trading, or the
      // wallet's inventory not yet scanned.
      equity_partial:
        accountSummary != null &&
        accountSummary.cash_balance != null &&
        !accountSummary.equity_complete,
      cash_balance: accountSummary?.cash_balance ?? null,
      marked_position_value: accountSummary?.position_value ?? 0,
      net_cash_flows_24h: accountSummary?.net_cash_flows_24h ?? 0,
      realized_pnl_24h: accountSummary?.realized_pnl_24h ?? null,
      unpriced_open_positions: accountSummary?.unpriced_open_positions ?? 0,
      unpriced_position_cost_basis:
        accountSummary?.unpriced_position_cost_basis ?? 0,
      total_pnl_percent:
        totalValue != null && totalValue > 0 && totalUnrealizedPnl != null
          ? (totalUnrealizedPnl / totalValue) * 100
          : undefined,
      total_unrealized_pnl: totalUnrealizedPnl,
      active_positions: accountSummary?.open_positions ?? positionsSummary?.open_positions ?? null,
      win_rate: positionsSummary?.win_rate ?? null,
    };
  }, [accountSummary, positionsSummary]);
  const { activities, status: activityStatus, unreadCount } = useActivity();

  const equityCurve = useMemo(() => {
    const points = accountHistory?.equity_curve ?? [];
    return points.map((point) => {
      const label = new Date(point.snapshot_time).toISOString();
      return {
        time: label,
        value: point.total_equity,
      };
    });
  }, [accountHistory]);

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
          <LiveIndicator
            stale={anyStale(
              positionsSummaryQuery,
              accountSummaryQuery,
              accountHistoryQuery,
              dynamicTunerQuery,
              tradeFlowQuery,
            )}
          />
        </div>
      </div>

      <PageIntro
        title="What this dashboard shows"
        description="This is your high-level system summary. It tells you whether the bot is healthy, what your open trades look like, and what the system has done most recently."
        bullets={[
          "Start with the status strip to confirm trading is safe and the scanner is active.",
          "Portfolio metrics summarize reconciled wallet cash and observed inventory exposure.",
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

      {tradeFlowSummary && (
        <Card>
          <CardContent className="p-4">
            <div className="grid gap-3 md:grid-cols-4">
              <div>
                <p className="text-xs text-muted-foreground">Trade Flow Conversion</p>
                <p className="text-lg font-semibold tabular-nums">
                  {tradeFlowSummary.total_generated_signals > 0
                    ? `${((tradeFlowSummary.total_executed_signals / tradeFlowSummary.total_generated_signals) * 100).toFixed(1)}%`
                    : "—"}
                </p>
              </div>
              <div>
                <p className="text-xs text-muted-foreground">Exit Ready</p>
                <p className="text-lg font-semibold tabular-nums">
                  {tradeFlowSummary.total_exit_ready_positions}
                </p>
              </div>
              <div>
                <p className="text-xs text-muted-foreground">Failed Positions</p>
                <p className="text-lg font-semibold tabular-nums">
                  {tradeFlowSummary.total_entry_failed_positions +
                    tradeFlowSummary.total_exit_failed_positions}
                </p>
              </div>
              <div>
                <p className="text-xs text-muted-foreground">Realized P&L Window</p>
                <p
                  className={cn(
                    "text-lg font-semibold tabular-nums",
                    tradeFlowSummary.total_realized_pnl >= 0 ? "text-profit" : "text-loss",
                  )}
                >
                  {formatCurrency(tradeFlowSummary.total_realized_pnl, {
                    showSign: true,
                  })}
                </p>
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Stats Grid */}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <MetricCard
          title={stats.equity_partial ? "Account Equity (partial)" : "Account Equity"}
          value={
            stats.cash_unavailable ? "Unavailable" : formatCurrencyOrDash(stats.total_value)
          }
          change={stats.total_pnl_percent}
          changeLabel={
            (stats.equity_partial
              ? "Partial valuation: not trading live, or the wallet's inventory scan has not completed, so shares outside tracked positions may be missing and the drawdown guard does not use it. "
              : "") +
            (stats.cash_unavailable
              ? `No cash balance: no trading wallet is set, or its balance could not be read. Reconciled exposure ${formatCurrency(stats.marked_position_value)}.`
              : `Cash ${formatCurrencyOrDash(stats.cash_balance)} + reconciled exposure ${formatCurrency(stats.marked_position_value)}.`) +
            ` Net cash flows 24h ${formatCurrency(stats.net_cash_flows_24h, { showSign: true })}.` +
            (stats.unpriced_open_positions > 0
              ? ` ${stats.unpriced_open_positions} item${stats.unpriced_open_positions === 1 ? "" : "s"} are valued from cost basis because no live mark is stored (cost basis ${formatCurrency(stats.unpriced_position_cost_basis)}).`
              : "")
          }
          trend={
            stats.total_pnl_percent == null
              ? "neutral"
              : stats.total_pnl_percent >= 0
                ? "up"
                : "down"
          }
        />
        <MetricCard
          title="Realized P&L 24h"
          value={formatCurrencyOrDash(stats.realized_pnl_24h, { showSign: true })}
          trend={
            stats.realized_pnl_24h == null
              ? "neutral"
              : stats.realized_pnl_24h >= 0
                ? "up"
                : "down"
          }
        />
        <MetricCard
          title="Unrealized P&L"
          value={formatCurrencyOrDash(stats.total_unrealized_pnl, { showSign: true })}
          trend={
            stats.total_unrealized_pnl != null && stats.total_unrealized_pnl > 0
              ? "up"
              : stats.total_unrealized_pnl != null && stats.total_unrealized_pnl < 0
                ? "down"
                : "neutral"
          }
        />
        <MetricCard
          title="Active Exposure"
          value={stats.active_positions != null ? stats.active_positions.toString() : "—"}
          changeLabel={
            stats.win_rate != null
              ? `Win rate: ${formatWinRatePercent(stats.win_rate, { input: "percent" })}`
              : undefined
          }
          trend="neutral"
        />
      </div>

      {/* Per-Strategy P&L Breakdown */}
      {tradeFlowSummary &&
        tradeFlowSummary.strategies &&
        tradeFlowSummary.strategies.length > 0 && (
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="flex items-center gap-2 text-sm font-medium text-muted-foreground">
                <BarChart2 className="h-4 w-4" />
                Strategy P&L
              </CardTitle>
            </CardHeader>
            <CardContent className="pb-4">
              <div className="space-y-2">
                {tradeFlowSummary.strategies
                  .filter((s) => s.closed_positions > 0 || s.open_positions > 0)
                  .sort((a, b) => b.net_pnl - a.net_pnl)
                  .map((s) => {
                    const label = STRATEGY_PNL_LABELS[s.strategy] || s.strategy;
                    return (
                      <div
                        key={s.strategy}
                        className="flex items-center justify-between rounded-md border px-3 py-2"
                      >
                        <div className="flex items-center gap-2">
                          <span className="text-sm font-medium">{label}</span>
                          <span className="text-xs text-muted-foreground">
                            {s.closed_positions} closed
                            {s.open_positions > 0 &&
                              ` / ${s.open_positions} open`}
                          </span>
                        </div>
                        <span
                          className={cn(
                            "text-sm font-semibold tabular-nums",
                            s.net_pnl > 0
                              ? "text-profit"
                              : s.net_pnl < 0
                                ? "text-loss"
                                : "text-muted-foreground",
                          )}
                        >
                          {formatCurrency(s.net_pnl, { showSign: true })}
                        </span>
                      </div>
                    );
                  })}
              </div>
            </CardContent>
          </Card>
        )}

      {accountSummary && (
        <Card>
          <CardContent className="p-4">
            <div className="grid gap-3 md:grid-cols-3">
              <div>
                <p className="text-xs text-muted-foreground">Inventory Sync</p>
                <p className="text-sm font-semibold">
                  {accountSummary.inventory_last_synced_at
                    ? formatTimeAgo(accountSummary.inventory_last_synced_at)
                    : "Pending first sync"}
                </p>
                <p className="text-xs text-muted-foreground">
                  Last scanned block{" "}
                  {accountSummary.inventory_last_scanned_block != null
                    ? accountSummary.inventory_last_scanned_block.toLocaleString()
                    : "—"}
                </p>
              </div>
              <div>
                <p className="text-xs text-muted-foreground">Historical Discovery</p>
                <p className="text-sm font-semibold">
                  {accountSummary.inventory_backfill_in_progress
                    ? "Backfill in progress"
                    : accountSummary.inventory_backfill_completed_at
                      ? "Backfill complete"
                      : "Awaiting scan"}
                </p>
                <p className="text-xs text-muted-foreground">
                  {accountSummary.inventory_backfill_in_progress &&
                  accountSummary.inventory_backfill_cursor_block != null
                    ? `Cursor block ${accountSummary.inventory_backfill_cursor_block.toLocaleString()}`
                    : accountSummary.inventory_backfill_completed_at
                      ? `Completed ${formatTimeAgo(accountSummary.inventory_backfill_completed_at)}`
                      : "The inventory scanner is still warming up."}
                </p>
              </div>
              <div>
                <p className="text-xs text-muted-foreground">Orphan Inventory</p>
                <p className="text-sm font-semibold">
                  {formatCurrency(accountSummary.orphan_marked_value)}
                </p>
                <p className="text-xs text-muted-foreground">
                  {accountSummary.orphan_positions} item
                  {accountSummary.orphan_positions === 1 ? "" : "s"} currently need explicit tracking or recovery
                </p>
              </div>
            </div>
          </CardContent>
        </Card>
      )}

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
                Account snapshots will appear here as the ledger builds
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
