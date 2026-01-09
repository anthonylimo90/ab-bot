'use client';

import { useState, useMemo } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { MetricCard } from '@/components/shared/MetricCard';
import { BacktestChart } from '@/components/charts/BacktestChart';
import { formatCurrency } from '@/lib/utils';
import { Play, ChevronDown, Loader2 } from 'lucide-react';

// Generate mock backtest equity curve
function generateBacktestData(
  startDate: string,
  endDate: string,
  capital: number,
  returnPercent: number
) {
  const data: { time: string; value: number }[] = [];
  const start = new Date(startDate);
  const end = new Date(endDate);
  const days = Math.ceil((end.getTime() - start.getTime()) / (1000 * 60 * 60 * 24));
  const dailyReturn = Math.pow(1 + returnPercent / 100, 1 / days) - 1;

  let value = capital;
  for (let i = 0; i <= days; i++) {
    const date = new Date(start);
    date.setDate(date.getDate() + i);

    // Add volatility
    const randomFactor = 1 + (Math.random() - 0.5) * 0.03;
    value = value * (1 + dailyReturn) * randomFactor;

    // Simulate a drawdown period
    if (i > days * 0.3 && i < days * 0.4) {
      value = value * 0.995;
    }

    data.push({
      time: date.toISOString().split('T')[0],
      value: Math.round(value * 100) / 100,
    });
  }
  return data;
}

// Mock results
const mockResults = {
  totalReturn: 342,
  totalReturnPercent: 34.2,
  sharpe: 1.85,
  maxDrawdown: -12.3,
  winRate: 67,
  totalTrades: 89,
};

export default function BacktestPage() {
  const [isRunning, setIsRunning] = useState(false);
  const [hasResults, setHasResults] = useState(false);
  const [capital, setCapital] = useState(1000);
  const [startDate, setStartDate] = useState('2024-01-01');
  const [endDate, setEndDate] = useState('2024-12-31');
  const [slippage, setSlippage] = useState(0.1);
  const [fees, setFees] = useState(0.1);

  // Generate backtest data when results are available
  const backtestData = useMemo(() => {
    if (!hasResults) return [];
    return generateBacktestData(startDate, endDate, capital, mockResults.totalReturnPercent);
  }, [hasResults, startDate, endDate, capital]);

  const runBacktest = () => {
    setIsRunning(true);
    setHasResults(false);
    // Simulate backtest
    setTimeout(() => {
      setIsRunning(false);
      setHasResults(true);
    }, 2000);
  };

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
        <Button onClick={runBacktest} disabled={isRunning}>
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
              <label className="text-sm font-medium">Period</label>
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
            <Button variant="outline" className="w-full justify-between">
              Select wallets... <ChevronDown className="h-4 w-4" />
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* Results */}
      {hasResults && (
        <>
          <div className="grid gap-4 md:grid-cols-4">
            <MetricCard
              title="Total Return"
              value={`+${mockResults.totalReturnPercent}%`}
              changeLabel={formatCurrency(mockResults.totalReturn)}
              trend="up"
            />
            <MetricCard
              title="Sharpe Ratio"
              value={mockResults.sharpe.toFixed(2)}
              trend="neutral"
            />
            <MetricCard
              title="Max Drawdown"
              value={`${mockResults.maxDrawdown}%`}
              trend="down"
            />
            <MetricCard
              title="Win Rate"
              value={`${mockResults.winRate}%`}
              changeLabel={`${mockResults.totalTrades} trades`}
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

          {/* Monthly Returns */}
          <Card>
            <CardHeader>
              <CardTitle>Monthly Returns</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="grid grid-cols-6 md:grid-cols-12 gap-2">
                {['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'].map(
                  (month, i) => {
                    const returnVal = (Math.random() - 0.3) * 15;
                    return (
                      <div
                        key={month}
                        className="text-center p-2 rounded bg-muted/50"
                      >
                        <div className="text-xs text-muted-foreground">{month}</div>
                        <div
                          className={`text-sm font-medium tabular-nums ${
                            returnVal >= 0 ? 'text-profit' : 'text-loss'
                          }`}
                        >
                          {returnVal >= 0 ? '+' : ''}
                          {returnVal.toFixed(1)}%
                        </div>
                      </div>
                    );
                  }
                )}
              </div>
            </CardContent>
          </Card>
        </>
      )}

      {!hasResults && !isRunning && (
        <Card>
          <CardContent className="py-20">
            <p className="text-center text-muted-foreground">
              Configure your backtest parameters and click &quot;Run Backtest&quot; to see
              results
            </p>
          </CardContent>
        </Card>
      )}

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
    </div>
  );
}
