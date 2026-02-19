"use client";

import { useState, useMemo, useEffect } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { MetricCard } from "@/components/shared/MetricCard";
import { BacktestChart } from "@/components/charts/BacktestChart";
import { useBacktest } from "@/hooks/useBacktest";
import { formatCurrency, cn, shortenAddress } from "@/lib/utils";
import type { StrategyConfig } from "@/types/api";
import {
  Play,
  Loader2,
  AlertCircle,
  History,
  ChevronLeft,
  ChevronRight,
} from "lucide-react";

type StrategyType = "arbitrage" | "momentum" | "mean_reversion" | "grid";

const STRATEGY_LABELS: Record<StrategyType, string> = {
  arbitrage: "Arbitrage",
  momentum: "Momentum",
  mean_reversion: "Mean Reversion",
  grid: "Grid",
};

const TRADE_LOG_PAGE_SIZE = 20;

export default function BacktestPage() {
  const {
    results,
    history,
    isRunning,
    error,
    runBacktest,
    loadHistory,
    loadResult,
    clearResults,
  } = useBacktest();

  // Date helpers
  const formatDate = (date: Date) => date.toISOString().split("T")[0];
  const today = useMemo(() => new Date(), []);
  const getDateDaysAgo = (days: number) => {
    const date = new Date(today);
    date.setDate(today.getDate() - days);
    return date;
  };

  // Form state
  const [capital, setCapital] = useState(1000);
  const [startDate, setStartDate] = useState(() =>
    formatDate(getDateDaysAgo(30)),
  );
  const [endDate, setEndDate] = useState(() => formatDate(today));
  const [slippage, setSlippage] = useState(0.1);
  const [fees, setFees] = useState(2.0);

  // Strategy state
  const [strategyType, setStrategyType] = useState<StrategyType>("arbitrage");

  // Arbitrage params
  const [minSpread, setMinSpread] = useState(2.0);
  const [maxPosition, setMaxPosition] = useState(1000);

  // Momentum params
  const [lookbackHours, setLookbackHours] = useState(24);
  const [momentumThreshold, setMomentumThreshold] = useState(5.0);
  const [momentumPositionSize, setMomentumPositionSize] = useState(10.0);

  // Mean Reversion params
  const [windowHours, setWindowHours] = useState(48);
  const [stdThreshold, setStdThreshold] = useState(2.0);
  const [mrPositionSize, setMrPositionSize] = useState(10.0);

  // Grid params
  const [gridLevels, setGridLevels] = useState(5);
  const [gridSpacingPct, setGridSpacingPct] = useState(2.0);
  const [orderSize, setOrderSize] = useState(5.0);

  // Elapsed timer
  const [elapsedSeconds, setElapsedSeconds] = useState(0);

  // Trade log pagination
  const [tradeLogPage, setTradeLogPage] = useState(0);

  // Date presets
  const datePresets = useMemo(
    () => [
      { label: "7D", days: 7 },
      { label: "30D", days: 30 },
      { label: "90D", days: 90 },
      {
        label: "YTD",
        days: Math.ceil(
          (today.getTime() - new Date(today.getFullYear(), 0, 1).getTime()) /
            (1000 * 60 * 60 * 24),
        ),
      },
    ],
    [today],
  );

  const applyDatePreset = (days: number) => {
    setStartDate(formatDate(getDateDaysAgo(days)));
    setEndDate(formatDate(today));
  };

  // Load history on mount
  useEffect(() => {
    loadHistory();
  }, [loadHistory]);

  // Elapsed timer
  useEffect(() => {
    if (!isRunning) {
      setElapsedSeconds(0);
      return;
    }
    const interval = setInterval(() => setElapsedSeconds((s) => s + 1), 1000);
    return () => clearInterval(interval);
  }, [isRunning]);

  // Build strategy config from form state
  const buildStrategyConfig = (): StrategyConfig => {
    switch (strategyType) {
      case "arbitrage":
        return {
          type: "arbitrage",
          min_spread: minSpread / 100,
          max_position: maxPosition,
        };
      case "momentum":
        return {
          type: "momentum",
          lookback_hours: lookbackHours,
          threshold: momentumThreshold / 100,
          position_size: momentumPositionSize / 100,
        };
      case "mean_reversion":
        return {
          type: "mean_reversion",
          window_hours: windowHours,
          std_threshold: stdThreshold,
          position_size: mrPositionSize / 100,
        };
      case "grid":
        return {
          type: "grid",
          grid_levels: gridLevels,
          grid_spacing_pct: gridSpacingPct / 100,
          order_size: orderSize / 100,
        };
    }
  };

  // Handle backtest run
  const handleRunBacktest = async () => {
    setTradeLogPage(0);
    await runBacktest({
      strategy: buildStrategyConfig(),
      start_date: startDate + "T00:00:00Z",
      end_date: endDate + "T00:00:00Z",
      initial_capital: capital,
      slippage_model:
        slippage > 0
          ? { type: "fixed", pct: slippage / 100 }
          : { type: "none" },
      fee_pct: fees / 100,
    });
  };

  // Generate equity curve from results
  const backtestData = useMemo(() => {
    if (!results?.equity_curve) return [];
    return results.equity_curve.map((point) => ({
      time: point.timestamp,
      value: point.value,
    }));
  }, [results]);

  // Paginated trade log
  const paginatedTradeLog = useMemo(() => {
    if (!results?.trade_log) return [];
    const start = tradeLogPage * TRADE_LOG_PAGE_SIZE;
    return results.trade_log.slice(start, start + TRADE_LOG_PAGE_SIZE);
  }, [results, tradeLogPage]);

  const totalTradeLogPages = results?.trade_log
    ? Math.ceil(results.trade_log.length / TRADE_LOG_PAGE_SIZE)
    : 0;

  return (
    <div className="space-y-5 sm:space-y-6">
      {/* Page Header */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h1 className="text-2xl font-bold tracking-tight sm:text-3xl">Backtest</h1>
          <p className="text-muted-foreground">
            Test strategies against historical data
          </p>
        </div>
        <Button onClick={handleRunBacktest} disabled={isRunning} className="w-full sm:w-auto">
          {isRunning ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Play className="mr-2 h-4 w-4" />
          )}
          {isRunning ? "Running..." : "Run Backtest"}
        </Button>
      </div>

      {/* Configuration */}
      <Card>
        <CardHeader>
          <CardTitle>Configuration</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          {/* Strategy Selector */}
          <div className="space-y-2">
            <label className="text-sm font-medium">Strategy</label>
            <div className="flex gap-1 flex-wrap">
              {(
                ["arbitrage", "momentum", "mean_reversion", "grid"] as const
              ).map((s) => (
                <Button
                  key={s}
                  variant={strategyType === s ? "default" : "outline"}
                  size="sm"
                  onClick={() => {
                    setStrategyType(s);
                    clearResults();
                  }}
                >
                  {STRATEGY_LABELS[s]}
                </Button>
              ))}
            </div>
          </div>

          {/* Main config grid */}
          <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
            <div className="space-y-2">
              <label className="text-sm font-medium">Initial Capital</label>
              <div className="flex items-center border rounded-md">
                <span className="px-3 text-muted-foreground">$</span>
                <input
                  type="number"
                  value={capital}
                  onChange={(e) => setCapital(Number(e.target.value))}
                  className="flex-1 bg-transparent py-2 pr-3 outline-none"
                />
              </div>
            </div>
            <div className="space-y-2">
              <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                <label className="text-sm font-medium">Period</label>
                <div className="flex flex-wrap gap-1">
                  {datePresets.map((preset) => (
                    <Button
                      key={preset.label}
                      variant="ghost"
                      size="sm"
                      className="h-6 px-2 text-xs"
                      onClick={() => applyDatePreset(preset.days)}
                    >
                      {preset.label}
                    </Button>
                  ))}
                </div>
              </div>
              <div className="flex flex-col gap-2 sm:flex-row">
                <input
                  type="date"
                  value={startDate}
                  onChange={(e) => setStartDate(e.target.value)}
                  className="flex-1 rounded-md border bg-transparent px-3 py-2"
                />
                <input
                  type="date"
                  value={endDate}
                  onChange={(e) => setEndDate(e.target.value)}
                  className="flex-1 rounded-md border bg-transparent px-3 py-2"
                />
              </div>
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">Slippage</label>
              <div className="flex items-center border rounded-md">
                <input
                  type="number"
                  value={slippage}
                  onChange={(e) => setSlippage(Number(e.target.value))}
                  step={0.01}
                  className="flex-1 bg-transparent py-2 pl-3 outline-none"
                />
                <span className="px-3 text-muted-foreground">%</span>
              </div>
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">Fees</label>
              <div className="flex items-center border rounded-md">
                <input
                  type="number"
                  value={fees}
                  onChange={(e) => setFees(Number(e.target.value))}
                  step={0.01}
                  className="flex-1 bg-transparent py-2 pl-3 outline-none"
                />
                <span className="px-3 text-muted-foreground">%</span>
              </div>
            </div>
          </div>

          {/* Strategy-specific parameters */}
          <div className="border-t pt-4">
            <label className="text-sm font-medium text-muted-foreground mb-3 block">
              {STRATEGY_LABELS[strategyType]} Parameters
            </label>
            <div className="grid gap-4 md:grid-cols-3">
              {strategyType === "arbitrage" && (
                <>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">
                      Min Spread (%)
                    </label>
                    <div className="flex items-center border rounded-md">
                      <input
                        type="number"
                        value={minSpread}
                        onChange={(e) => setMinSpread(Number(e.target.value))}
                        step={0.1}
                        className="flex-1 bg-transparent py-2 pl-3 outline-none"
                      />
                      <span className="px-3 text-muted-foreground">%</span>
                    </div>
                    <p className="text-xs text-muted-foreground">
                      Minimum spread to trigger a trade
                    </p>
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">
                      Max Position ($)
                    </label>
                    <div className="flex items-center border rounded-md">
                      <span className="px-3 text-muted-foreground">$</span>
                      <input
                        type="number"
                        value={maxPosition}
                        onChange={(e) => setMaxPosition(Number(e.target.value))}
                        className="flex-1 bg-transparent py-2 pr-3 outline-none"
                      />
                    </div>
                    <p className="text-xs text-muted-foreground">
                      Maximum size per position
                    </p>
                  </div>
                </>
              )}
              {strategyType === "momentum" && (
                <>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">
                      Lookback (hours)
                    </label>
                    <input
                      type="number"
                      value={lookbackHours}
                      onChange={(e) => setLookbackHours(Number(e.target.value))}
                      className="w-full rounded-md border bg-transparent px-3 py-2"
                    />
                    <p className="text-xs text-muted-foreground">
                      Price momentum lookback period
                    </p>
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">Threshold (%)</label>
                    <div className="flex items-center border rounded-md">
                      <input
                        type="number"
                        value={momentumThreshold}
                        onChange={(e) =>
                          setMomentumThreshold(Number(e.target.value))
                        }
                        step={0.1}
                        className="flex-1 bg-transparent py-2 pl-3 outline-none"
                      />
                      <span className="px-3 text-muted-foreground">%</span>
                    </div>
                    <p className="text-xs text-muted-foreground">
                      Momentum required to trigger entry
                    </p>
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">
                      Position Size (%)
                    </label>
                    <div className="flex items-center border rounded-md">
                      <input
                        type="number"
                        value={momentumPositionSize}
                        onChange={(e) =>
                          setMomentumPositionSize(Number(e.target.value))
                        }
                        step={1}
                        className="flex-1 bg-transparent py-2 pl-3 outline-none"
                      />
                      <span className="px-3 text-muted-foreground">%</span>
                    </div>
                    <p className="text-xs text-muted-foreground">
                      Fraction of portfolio per trade
                    </p>
                  </div>
                </>
              )}
              {strategyType === "mean_reversion" && (
                <>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">
                      Window (hours)
                    </label>
                    <input
                      type="number"
                      value={windowHours}
                      onChange={(e) => setWindowHours(Number(e.target.value))}
                      className="w-full rounded-md border bg-transparent px-3 py-2"
                    />
                    <p className="text-xs text-muted-foreground">
                      Moving average window for mean calculation
                    </p>
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">Std Threshold</label>
                    <input
                      type="number"
                      value={stdThreshold}
                      onChange={(e) => setStdThreshold(Number(e.target.value))}
                      step={0.1}
                      className="w-full rounded-md border bg-transparent px-3 py-2"
                    />
                    <p className="text-xs text-muted-foreground">
                      Standard deviations below mean to trigger entry
                    </p>
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">
                      Position Size (%)
                    </label>
                    <div className="flex items-center border rounded-md">
                      <input
                        type="number"
                        value={mrPositionSize}
                        onChange={(e) =>
                          setMrPositionSize(Number(e.target.value))
                        }
                        step={1}
                        className="flex-1 bg-transparent py-2 pl-3 outline-none"
                      />
                      <span className="px-3 text-muted-foreground">%</span>
                    </div>
                    <p className="text-xs text-muted-foreground">
                      Fraction of portfolio per trade
                    </p>
                  </div>
                </>
              )}
              {strategyType === "grid" && (
                <>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">Grid Levels</label>
                    <input
                      type="number"
                      value={gridLevels}
                      onChange={(e) => setGridLevels(Number(e.target.value))}
                      min={1}
                      className="w-full rounded-md border bg-transparent px-3 py-2"
                    />
                    <p className="text-xs text-muted-foreground">
                      Number of buy levels below center price
                    </p>
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">Spacing (%)</label>
                    <div className="flex items-center border rounded-md">
                      <input
                        type="number"
                        value={gridSpacingPct}
                        onChange={(e) =>
                          setGridSpacingPct(Number(e.target.value))
                        }
                        step={0.1}
                        className="flex-1 bg-transparent py-2 pl-3 outline-none"
                      />
                      <span className="px-3 text-muted-foreground">%</span>
                    </div>
                    <p className="text-xs text-muted-foreground">
                      Price distance between grid levels
                    </p>
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">
                      Order Size (%)
                    </label>
                    <div className="flex items-center border rounded-md">
                      <input
                        type="number"
                        value={orderSize}
                        onChange={(e) => setOrderSize(Number(e.target.value))}
                        step={1}
                        className="flex-1 bg-transparent py-2 pl-3 outline-none"
                      />
                      <span className="px-3 text-muted-foreground">%</span>
                    </div>
                    <p className="text-xs text-muted-foreground">
                      Portfolio fraction per grid order
                    </p>
                  </div>
                </>
              )}
            </div>
          </div>
        </CardContent>
      </Card>

      {/* Error State */}
      {error && (
        <Card className="border-destructive">
          <CardContent className="p-6">
            <div className="flex items-center gap-4">
              <AlertCircle className="h-8 w-8 text-destructive" />
              <div>
                <h3 className="font-medium">Backtest Failed</h3>
                <p className="text-sm text-muted-foreground">{error}</p>
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Running State */}
      {isRunning && (
        <Card>
          <CardContent className="py-20">
            <div className="flex flex-col items-center gap-4">
              <Loader2 className="h-8 w-8 animate-spin text-primary" />
              <p className="text-muted-foreground">Running backtest…</p>
              <p className="text-xs text-muted-foreground">
                Simulating{" "}
                {Math.ceil(
                  (new Date(endDate).getTime() -
                    new Date(startDate).getTime()) /
                    (1000 * 60 * 60 * 24),
                )}{" "}
                days of trading &middot; {elapsedSeconds}s elapsed
              </p>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Results */}
      {results && results.status === "completed" && (
        <>
          {/* Primary metrics */}
          <div className="grid gap-4 md:grid-cols-4">
            <MetricCard
              title="Total Return"
              value={`${results.total_return_pct >= 0 ? "+" : ""}${results.total_return_pct.toFixed(1)}%`}
              changeLabel={formatCurrency(results.total_return)}
              trend={results.total_return_pct >= 0 ? "up" : "down"}
            />
            <MetricCard
              title="Sharpe Ratio"
              value={results.sharpe_ratio.toFixed(2)}
              trend="neutral"
            />
            <MetricCard
              title="Max Drawdown"
              value={`${results.max_drawdown_pct.toFixed(1)}%`}
              trend="down"
            />
            <MetricCard
              title="Win Rate"
              value={`${results.win_rate.toFixed(0)}%`}
              changeLabel={`${results.total_trades} trades`}
              trend="neutral"
            />
          </div>

          {/* Extended metrics (if available) */}
          {results.calmar_ratio != null && (
            <div className="grid gap-4 md:grid-cols-4">
              <MetricCard
                title="Calmar Ratio"
                value={results.calmar_ratio.toFixed(2)}
                trend="neutral"
              />
              <MetricCard
                title="Expectancy"
                value={
                  results.expectancy != null
                    ? formatCurrency(results.expectancy)
                    : "\u2014"
                }
                trend="neutral"
              />
              <MetricCard
                title="VaR 95%"
                value={
                  results.var_95 != null
                    ? `${(results.var_95 * 100).toFixed(2)}%`
                    : "\u2014"
                }
                trend="down"
              />
              <MetricCard
                title="Avg Duration"
                value={
                  results.avg_trade_duration_hours != null
                    ? `${results.avg_trade_duration_hours.toFixed(1)}h`
                    : "\u2014"
                }
                trend="neutral"
              />
            </div>
          )}

          {/* Equity Curve */}
          <Card>
            <CardHeader>
              <CardTitle>Equity Curve</CardTitle>
            </CardHeader>
            <CardContent>
              <BacktestChart
                data={backtestData}
                height={350}
                baseline={capital}
              />
            </CardContent>
          </Card>

          {/* Performance Breakdown */}
          <Card>
            <CardHeader>
              <CardTitle>Performance Breakdown</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
                <div>
                  <p className="text-sm text-muted-foreground">Final Value</p>
                  <p className="font-medium tabular-nums">
                    {formatCurrency(results.final_value)}
                  </p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Total Fees</p>
                  <p className="font-medium tabular-nums text-loss">
                    {formatCurrency(results.total_fees)}
                  </p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Profit Factor</p>
                  <p className="font-medium tabular-nums">
                    {results.profit_factor.toFixed(2)}
                  </p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Sortino Ratio</p>
                  <p className="font-medium tabular-nums">
                    {results.sortino_ratio.toFixed(2)}
                  </p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Avg Win</p>
                  <p className="font-medium tabular-nums text-profit">
                    {formatCurrency(results.avg_win)}
                  </p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Avg Loss</p>
                  <p className="font-medium tabular-nums text-loss">
                    {formatCurrency(results.avg_loss)}
                  </p>
                </div>
                {results.best_trade_return != null && (
                  <div>
                    <p className="text-sm text-muted-foreground">Best Trade</p>
                    <p className="font-medium tabular-nums text-profit">
                      {(results.best_trade_return * 100).toFixed(1)}%
                    </p>
                  </div>
                )}
                {results.worst_trade_return != null && (
                  <div>
                    <p className="text-sm text-muted-foreground">Worst Trade</p>
                    <p className="font-medium tabular-nums text-loss">
                      {(results.worst_trade_return * 100).toFixed(1)}%
                    </p>
                  </div>
                )}
                {results.recovery_factor != null && (
                  <div>
                    <p className="text-sm text-muted-foreground">
                      Recovery Factor
                    </p>
                    <p className="font-medium tabular-nums">
                      {results.recovery_factor.toFixed(2)}
                    </p>
                  </div>
                )}
                {results.max_consecutive_wins != null && (
                  <div>
                    <p className="text-sm text-muted-foreground">
                      Max Consec. Wins
                    </p>
                    <p className="font-medium tabular-nums text-profit">
                      {results.max_consecutive_wins}
                    </p>
                  </div>
                )}
                {results.max_consecutive_losses != null && (
                  <div>
                    <p className="text-sm text-muted-foreground">
                      Max Consec. Losses
                    </p>
                    <p className="font-medium tabular-nums text-loss">
                      {results.max_consecutive_losses}
                    </p>
                  </div>
                )}
                {results.cvar_95 != null && (
                  <div>
                    <p className="text-sm text-muted-foreground">CVaR 95%</p>
                    <p className="font-medium tabular-nums">
                      {(results.cvar_95 * 100).toFixed(2)}%
                    </p>
                  </div>
                )}
              </div>
            </CardContent>
          </Card>

          {/* Trade Log */}
          {results.trade_log && results.trade_log.length > 0 && (
            <Card>
              <CardHeader>
                <CardTitle>
                  Trade Log ({results.trade_log.length} trades)
                </CardTitle>
              </CardHeader>
              <CardContent>
                <div className="overflow-x-auto">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b text-muted-foreground">
                        <th className="text-left py-2 pr-4 font-medium">
                          Market
                        </th>
                        <th className="text-left py-2 pr-4 font-medium">
                          Type
                        </th>
                        <th className="text-right py-2 pr-4 font-medium">
                          Entry
                        </th>
                        <th className="text-right py-2 pr-4 font-medium">
                          Exit
                        </th>
                        <th className="text-right py-2 pr-4 font-medium">
                          Qty
                        </th>
                        <th className="text-right py-2 pr-4 font-medium">
                          P&L
                        </th>
                        <th className="text-right py-2 font-medium">Return</th>
                      </tr>
                    </thead>
                    <tbody>
                      {paginatedTradeLog.map((trade, i) => (
                        <tr
                          key={`${trade.entry_time}-${i}`}
                          className="border-b border-border/50"
                        >
                          <td className="py-2 pr-4">
                            {shortenAddress(trade.market_id, 6)}
                          </td>
                          <td className="py-2 pr-4">
                            <span
                              className={cn(
                                "inline-block rounded px-1.5 py-0.5 text-xs font-medium",
                                trade.trade_type === "buy"
                                  ? "bg-profit/10 text-profit"
                                  : "bg-muted text-muted-foreground",
                              )}
                            >
                              {trade.trade_type}
                            </span>
                          </td>
                          <td className="py-2 pr-4 text-right tabular-nums">
                            ${trade.entry_price.toFixed(4)}
                          </td>
                          <td className="py-2 pr-4 text-right tabular-nums">
                            {trade.exit_price != null
                              ? `$${trade.exit_price.toFixed(4)}`
                              : "\u2014"}
                          </td>
                          <td className="py-2 pr-4 text-right tabular-nums">
                            {trade.quantity.toFixed(2)}
                          </td>
                          <td
                            className={cn(
                              "py-2 pr-4 text-right tabular-nums",
                              trade.pnl != null && trade.pnl >= 0
                                ? "text-profit"
                                : "text-loss",
                            )}
                          >
                            {trade.pnl != null
                              ? formatCurrency(trade.pnl, { showSign: true })
                              : "\u2014"}
                          </td>
                          <td
                            className={cn(
                              "py-2 text-right tabular-nums",
                              trade.return_pct != null && trade.return_pct >= 0
                                ? "text-profit"
                                : "text-loss",
                            )}
                          >
                            {trade.return_pct != null
                              ? `${trade.return_pct >= 0 ? "+" : ""}${(trade.return_pct * 100).toFixed(1)}%`
                              : "\u2014"}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                {totalTradeLogPages > 1 && (
                  <div className="flex flex-col gap-2 pt-4 sm:flex-row sm:items-center sm:justify-between">
                    <p className="text-xs text-muted-foreground">
                      Page {tradeLogPage + 1} of {totalTradeLogPages}
                    </p>
                    <div className="flex gap-1">
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() =>
                          setTradeLogPage((p) => Math.max(0, p - 1))
                        }
                        disabled={tradeLogPage === 0}
                      >
                        <ChevronLeft className="h-4 w-4" />
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() =>
                          setTradeLogPage((p) =>
                            Math.min(totalTradeLogPages - 1, p + 1),
                          )
                        }
                        disabled={tradeLogPage >= totalTradeLogPages - 1}
                      >
                        <ChevronRight className="h-4 w-4" />
                      </Button>
                    </div>
                  </div>
                )}
              </CardContent>
            </Card>
          )}
        </>
      )}

      {/* No Results Yet */}
      {!results && !isRunning && !error && (
        <Card>
          <CardContent className="py-20">
            <p className="text-center text-muted-foreground">
              Configure your backtest parameters and click &quot;Run
              Backtest&quot; to see results
            </p>
          </CardContent>
        </Card>
      )}

      {/* History */}
      {history.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <History className="h-5 w-5" />
              Backtest History
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="space-y-2">
              {history
                .filter((r) => r.status === "completed")
                .slice(0, 10)
                .map((result) => (
                  <button
                    key={result.id}
                    type="button"
                    className="flex w-full flex-col gap-2 rounded-lg bg-muted/30 p-3 text-left transition-colors hover:bg-muted/50 sm:flex-row sm:items-center sm:justify-between"
                    onClick={() => {
                      setTradeLogPage(0);
                      loadResult(result.id);
                    }}
                  >
                    <div>
                      <p className="font-medium text-sm">
                        {STRATEGY_LABELS[
                          result.strategy.type as StrategyType
                        ] ?? result.strategy.type}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {new Date(result.created_at).toLocaleDateString()} |{" "}
                        {result.start_date?.split("T")[0]} to{" "}
                        {result.end_date?.split("T")[0]}
                      </p>
                    </div>
                    <div className="text-right">
                      <p
                        className={cn(
                          "font-medium tabular-nums",
                          result.total_return_pct >= 0
                            ? "text-profit"
                            : "text-loss",
                        )}
                      >
                        {result.total_return_pct >= 0 ? "+" : ""}
                        {result.total_return_pct.toFixed(1)}%
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {result.total_trades} trades
                      </p>
                    </div>
                  </button>
                ))}
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
