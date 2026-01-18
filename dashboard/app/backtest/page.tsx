'use client';

import { useState, useMemo, useEffect } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { MetricCard } from '@/components/shared/MetricCard';
import { BacktestChart } from '@/components/charts/BacktestChart';
import { useBacktest } from '@/hooks/useBacktest';
import { useRosterStore } from '@/stores/roster-store';
import { formatCurrency, shortenAddress } from '@/lib/utils';
import { Play, ChevronDown, Loader2, AlertCircle, History } from 'lucide-react';

export default function BacktestPage() {
  const { activeWallets, benchWallets } = useRosterStore();
  const { results, history, isRunning, error, runBacktest, loadHistory, loadResult } = useBacktest();

  // Date helpers
  const formatDate = (date: Date) => date.toISOString().split('T')[0];
  const today = useMemo(() => new Date(), []);
  const getDateDaysAgo = (days: number) => {
    const date = new Date(today);
    date.setDate(today.getDate() - days);
    return date;
  };

  // Form state
  const [capital, setCapital] = useState(1000);
  const [startDate, setStartDate] = useState(() => formatDate(getDateDaysAgo(30)));
  const [endDate, setEndDate] = useState(() => formatDate(today));
  const [slippage, setSlippage] = useState(0.1);

  // Date presets
  const datePresets = useMemo(() => [
    { label: '7D', days: 7 },
    { label: '30D', days: 30 },
    { label: '90D', days: 90 },
    { label: 'YTD', days: Math.ceil((today.getTime() - new Date(today.getFullYear(), 0, 1).getTime()) / (1000 * 60 * 60 * 24)) },
  ], [today]);

  const applyDatePreset = (days: number) => {
    setStartDate(formatDate(getDateDaysAgo(days)));
    setEndDate(formatDate(today));
  };
  const [fees, setFees] = useState(0.1);
  const [selectedWallets, setSelectedWallets] = useState<string[]>([]);

  // Load history on mount
  useEffect(() => {
    loadHistory();
  }, [loadHistory]);

  // All tracked wallets
  const allWallets = useMemo(() => {
    return [...activeWallets, ...benchWallets];
  }, [activeWallets, benchWallets]);

  // Handle backtest run
  const handleRunBacktest = async () => {
    await runBacktest({
      strategy: {
        type: 'CopyTrading',
        wallets: selectedWallets.length > 0 ? selectedWallets : allWallets.map(w => w.address),
      },
      start_date: startDate,
      end_date: endDate,
      initial_capital: capital,
      slippage_pct: slippage,
      fee_pct: fees,
    });
  };

  // Toggle wallet selection
  const toggleWallet = (address: string) => {
    setSelectedWallets(prev =>
      prev.includes(address)
        ? prev.filter(a => a !== address)
        : [...prev, address]
    );
  };

  // Generate equity curve from results
  const backtestData = useMemo(() => {
    if (!results?.equity_curve) return [];
    return results.equity_curve.map(point => ({
      time: point.timestamp,
      value: point.value,
    }));
  }, [results]);

  return (
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Backtest</h1>
          <p className="text-muted-foreground">
            Test strategies against historical data
          </p>
        </div>
        <Button onClick={handleRunBacktest} disabled={isRunning}>
          {isRunning ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Play className="mr-2 h-4 w-4" />
          )}
          {isRunning ? 'Running...' : 'Run Backtest'}
        </Button>
      </div>

      {/* Configuration */}
      <Card>
        <CardHeader>
          <CardTitle>Backtest Configuration</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
            <div className="space-y-2">
              <label className="text-sm font-medium">Strategy</label>
              <Button variant="outline" className="w-full justify-between">
                Copy Trading <ChevronDown className="h-4 w-4" />
              </Button>
            </div>
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
              <div className="flex items-center justify-between">
                <label className="text-sm font-medium">Period</label>
                <div className="flex gap-1">
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
              <div className="flex gap-2">
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
            <div className="space-y-2">
              <label className="text-sm font-medium">Allocation</label>
              <Button variant="outline" className="w-full justify-between">
                Equal Weight <ChevronDown className="h-4 w-4" />
              </Button>
            </div>
          </div>

          <div className="mt-4 space-y-2">
            <label className="text-sm font-medium">Wallets to Copy</label>
            {allWallets.length > 0 ? (
              <div className="flex flex-wrap gap-2">
                {allWallets.map((wallet) => (
                  <Button
                    key={wallet.address}
                    variant={selectedWallets.includes(wallet.address) ? 'default' : 'outline'}
                    size="sm"
                    onClick={() => toggleWallet(wallet.address)}
                  >
                    {wallet.label || shortenAddress(wallet.address)}
                  </Button>
                ))}
                {selectedWallets.length === 0 && (
                  <p className="text-sm text-muted-foreground ml-2">
                    No wallets selected - will use all tracked wallets
                  </p>
                )}
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">
                No wallets being tracked. Add wallets from the Trading page first.
              </p>
            )}
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

      {/* Results */}
      {results && results.status === 'completed' && (
        <>
          <div className="grid gap-4 md:grid-cols-4">
            <MetricCard
              title="Total Return"
              value={`${results.total_return_pct >= 0 ? '+' : ''}${results.total_return_pct.toFixed(1)}%`}
              changeLabel={formatCurrency(results.total_return)}
              trend={results.total_return_pct >= 0 ? 'up' : 'down'}
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

          {/* Additional Stats */}
          <Card>
            <CardHeader>
              <CardTitle>Performance Breakdown</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
                <div>
                  <p className="text-sm text-muted-foreground">Final Value</p>
                  <p className="font-medium">{formatCurrency(results.final_value)}</p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Total Fees</p>
                  <p className="font-medium text-loss">{formatCurrency(results.total_fees)}</p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Profit Factor</p>
                  <p className="font-medium">{results.profit_factor.toFixed(2)}</p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Sortino Ratio</p>
                  <p className="font-medium">{results.sortino_ratio.toFixed(2)}</p>
                </div>
              </div>
            </CardContent>
          </Card>
        </>
      )}

      {/* No Results Yet */}
      {!results && !isRunning && !error && (
        <Card>
          <CardContent className="py-20">
            <p className="text-center text-muted-foreground">
              Configure your backtest parameters and click &quot;Run Backtest&quot; to see
              results
            </p>
          </CardContent>
        </Card>
      )}

      {/* Running State */}
      {isRunning && (
        <Card>
          <CardContent className="py-20">
            <div className="flex flex-col items-center gap-4">
              <Loader2 className="h-8 w-8 animate-spin text-primary" />
              <p className="text-muted-foreground">Running backtest...</p>
              <p className="text-xs text-muted-foreground">
                Simulating {Math.ceil((new Date(endDate).getTime() - new Date(startDate).getTime()) / (1000 * 60 * 60 * 24))} days of trading
              </p>
            </div>
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
              {history.slice(0, 5).map((result) => (
                <div
                  key={result.id}
                  className="flex items-center justify-between p-3 rounded-lg bg-muted/30 hover:bg-muted/50 cursor-pointer transition-colors"
                  onClick={() => loadResult(result.id)}
                >
                  <div>
                    <p className="font-medium text-sm">
                      {result.strategy.type}
                    </p>
                    <p className="text-xs text-muted-foreground">
                      {new Date(result.created_at).toLocaleDateString()} |
                      {result.start_date} to {result.end_date}
                    </p>
                  </div>
                  <div className="text-right">
                    <p className={`font-medium ${result.total_return_pct >= 0 ? 'text-profit' : 'text-loss'}`}>
                      {result.total_return_pct >= 0 ? '+' : ''}{result.total_return_pct.toFixed(1)}%
                    </p>
                    <p className="text-xs text-muted-foreground">
                      {result.total_trades} trades
                    </p>
                  </div>
                </div>
              ))}
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
