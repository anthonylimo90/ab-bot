'use client';

import { useEffect, useState } from 'react';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { cn, formatCurrency, formatPercent } from '@/lib/utils';
import type { DemoPnlSimulation } from '@/types/api';
import api from '@/lib/api';

interface DemoPnlCalculatorProps {
  className?: string;
  initialAmount?: number;
}

type Period = '7d' | '30d' | '90d';

export function DemoPnlCalculator({
  className,
  initialAmount = 100,
}: DemoPnlCalculatorProps) {
  const [amount, setAmount] = useState(initialAmount);
  const [period, setPeriod] = useState<Period>('30d');
  const [simulation, setSimulation] = useState<DemoPnlSimulation | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const runSimulation = async () => {
    setIsLoading(true);
    try {
      const data = await api.simulateDemoPnl({ amount, period });
      setSimulation(data);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Simulation failed');
    } finally {
      setIsLoading(false);
    }
  };

  useEffect(() => {
    runSimulation();
  }, [amount, period]);

  const periods: { value: Period; label: string }[] = [
    { value: '7d', label: '7 Days' },
    { value: '30d', label: '30 Days' },
    { value: '90d', label: '90 Days' },
  ];

  const presetAmounts = [100, 500, 1000, 5000];

  return (
    <Card className={cn('', className)}>
      <CardHeader>
        <CardTitle className="text-lg">Demo P&L Simulator</CardTitle>
        <CardDescription>
          See how much you could have made copy trading top wallets
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-6">
        {/* Amount selector */}
        <div className="space-y-2">
          <label className="text-sm font-medium">Investment Amount</label>
          <div className="flex flex-wrap gap-2">
            {presetAmounts.map((preset) => (
              <Button
                key={preset}
                variant={amount === preset ? 'default' : 'outline'}
                size="sm"
                onClick={() => setAmount(preset)}
              >
                {formatCurrency(preset, { decimals: 0 })}
              </Button>
            ))}
          </div>
        </div>

        {/* Period selector */}
        <div className="space-y-2">
          <label className="text-sm font-medium">Time Period</label>
          <div className="flex gap-2">
            {periods.map(({ value, label }) => (
              <Button
                key={value}
                variant={period === value ? 'default' : 'outline'}
                size="sm"
                onClick={() => setPeriod(value)}
              >
                {label}
              </Button>
            ))}
          </div>
        </div>

        {/* Results */}
        {isLoading ? (
          <div className="flex items-center justify-center py-8">
            <div className="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent" />
          </div>
        ) : error ? (
          <div className="text-center text-sm text-muted-foreground py-4">
            {error}
          </div>
        ) : simulation && (
          <div className="space-y-4">
            {/* P&L Summary */}
            <div className="rounded-lg bg-muted/50 p-4">
              <div className="grid grid-cols-2 gap-4">
                <div>
                  <div className="text-xs text-muted-foreground">Initial</div>
                  <div className="text-lg font-semibold">
                    {formatCurrency(simulation.initial_amount)}
                  </div>
                </div>
                <div>
                  <div className="text-xs text-muted-foreground">Current Value</div>
                  <div className="text-lg font-semibold">
                    {formatCurrency(simulation.current_value)}
                  </div>
                </div>
              </div>
              <div className="mt-4 border-t border-border pt-4">
                <div className="flex items-center justify-between">
                  <span className="text-sm text-muted-foreground">Total P&L</span>
                  <div className="text-right">
                    <span
                      className={cn(
                        'text-xl font-bold',
                        simulation.pnl >= 0 ? 'text-profit' : 'text-loss'
                      )}
                    >
                      {formatCurrency(simulation.pnl, { showSign: true })}
                    </span>
                    <span
                      className={cn(
                        'ml-2 text-sm',
                        simulation.pnl >= 0 ? 'text-profit' : 'text-loss'
                      )}
                    >
                      ({formatPercent(simulation.pnl_pct, { showSign: true })})
                    </span>
                  </div>
                </div>
              </div>
            </div>

            {/* Mini equity curve */}
            {simulation.equity_curve.length > 0 && (
              <div className="space-y-2">
                <div className="text-xs text-muted-foreground">Equity Curve</div>
                <div className="h-16">
                  <MiniChart data={simulation.equity_curve} />
                </div>
              </div>
            )}

            {/* Wallet breakdown */}
            <div className="space-y-2">
              <div className="text-xs text-muted-foreground">Wallet Allocation</div>
              <div className="space-y-2">
                {simulation.wallets.map((wallet) => (
                  <div
                    key={wallet.address}
                    className="flex items-center justify-between text-sm"
                  >
                    <div className="flex items-center gap-2">
                      <div
                        className="h-2 w-2 rounded-full bg-primary"
                        style={{ opacity: wallet.allocation_pct / 100 + 0.3 }}
                      />
                      <span className="font-mono text-xs">{wallet.address}</span>
                      <span className="text-muted-foreground">
                        ({wallet.allocation_pct}%)
                      </span>
                    </div>
                    <span
                      className={cn(
                        'font-medium',
                        wallet.pnl >= 0 ? 'text-profit' : 'text-loss'
                      )}
                    >
                      {formatCurrency(wallet.pnl, { showSign: true })}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function MiniChart({ data }: { data: { date: string; value: number }[] }) {
  if (data.length === 0) return null;

  const values = data.map((d) => d.value);
  const min = Math.min(...values);
  const max = Math.max(...values);
  const range = max - min || 1;

  const points = data
    .map((d, i) => {
      const x = (i / (data.length - 1)) * 100;
      const y = 100 - ((d.value - min) / range) * 100;
      return `${x},${y}`;
    })
    .join(' ');

  const isPositive = data[data.length - 1].value >= data[0].value;

  return (
    <svg viewBox="0 0 100 100" preserveAspectRatio="none" className="w-full h-full">
      <polyline
        points={points}
        fill="none"
        stroke={isPositive ? 'hsl(var(--profit))' : 'hsl(var(--loss))'}
        strokeWidth="2"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

export default DemoPnlCalculator;
