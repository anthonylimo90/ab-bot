'use client';

import { useState, useMemo } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { EquityCurve } from '@/components/charts/EquityCurve';
import { formatCurrency, shortenAddress } from '@/lib/utils';
import {
  Star,
  Plus,
  ChevronDown,
} from 'lucide-react';

// Generate mock equity curve data
function generateEquityCurve(days: number, roi: number) {
  const data: { time: string; value: number }[] = [];
  let value = 100;
  const dailyReturn = Math.pow(1 + roi / 100, 1 / days) - 1;
  const now = new Date();

  for (let i = days; i >= 0; i--) {
    const date = new Date(now);
    date.setDate(date.getDate() - i);

    // Add some volatility around the trend
    const randomFactor = 1 + (Math.random() - 0.5) * 0.04;
    value = value * (1 + dailyReturn) * randomFactor;

    data.push({
      time: date.toISOString().split('T')[0],
      value: Math.round(value * 100) / 100,
    });
  }
  return data;
}

// Mock data
const mockWallets = [
  {
    address: '0x1234567890abcdef1234567890abcdef12345678',
    rank: 1,
    roi30d: 47.3,
    sharpe: 2.4,
    trades: 156,
    winRate: 71,
    maxDrawdown: -8.2,
    prediction: 'HIGH_POTENTIAL' as const,
    confidence: 85,
    tracked: false,
  },
  {
    address: '0xabcdef1234567890abcdef1234567890abcdef12',
    rank: 2,
    roi30d: 38.1,
    sharpe: 1.9,
    trades: 89,
    winRate: 68,
    maxDrawdown: -12.1,
    prediction: 'MODERATE' as const,
    confidence: 72,
    tracked: true,
  },
  {
    address: '0x5678901234abcdef5678901234abcdef56789012',
    rank: 3,
    roi30d: 29.4,
    sharpe: 1.5,
    trades: 234,
    winRate: 64,
    maxDrawdown: -15.3,
    prediction: 'MODERATE' as const,
    confidence: 65,
    tracked: false,
  },
  {
    address: '0x9876543210fedcba9876543210fedcba98765432',
    rank: 4,
    roi30d: 22.8,
    sharpe: 1.3,
    trades: 78,
    winRate: 61,
    maxDrawdown: -18.5,
    prediction: 'LOW_POTENTIAL' as const,
    confidence: 52,
    tracked: false,
  },
];

const predictionColors = {
  HIGH_POTENTIAL: 'text-profit bg-profit/10',
  MODERATE: 'text-yellow-500 bg-yellow-500/10',
  LOW_POTENTIAL: 'text-loss bg-loss/10',
  INSUFFICIENT_DATA: 'text-muted-foreground bg-muted',
};

const predictionLabels = {
  HIGH_POTENTIAL: 'High Potential',
  MODERATE: 'Moderate',
  LOW_POTENTIAL: 'Low Potential',
  INSUFFICIENT_DATA: 'Insufficient Data',
};

export default function DiscoverPage() {
  const [whatIfAmount, setWhatIfAmount] = useState(100);
  const [sortBy, setSortBy] = useState<'roi' | 'sharpe' | 'winRate'>('roi');
  const [timePeriod, setTimePeriod] = useState('30d');

  // Generate equity curves for each wallet (memoized to prevent regeneration on every render)
  const walletEquityCurves = useMemo(() => {
    return mockWallets.reduce((acc, wallet) => {
      acc[wallet.address] = generateEquityCurve(30, wallet.roi30d);
      return acc;
    }, {} as Record<string, { time: string; value: number }[]>);
  }, []);

  return (
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">
            Discover Strategies
          </h1>
          <p className="text-muted-foreground">
            Find top-performing wallets to copy
          </p>
        </div>
      </div>

      {/* Filters */}
      <Card>
        <CardContent className="p-4">
          <div className="flex flex-wrap items-center gap-4">
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">Sort by:</span>
              <Button variant="outline" size="sm">
                ROI <ChevronDown className="ml-1 h-4 w-4" />
              </Button>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">Time:</span>
              <Button variant="outline" size="sm">
                30 Days <ChevronDown className="ml-1 h-4 w-4" />
              </Button>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">Min Trades:</span>
              <Button variant="outline" size="sm">
                10 <ChevronDown className="ml-1 h-4 w-4" />
              </Button>
            </div>
            <div className="ml-auto flex items-center gap-4">
              <label className="flex items-center gap-2 text-sm">
                <input type="checkbox" className="rounded" defaultChecked />
                <span>Hide bots</span>
              </label>
              <label className="flex items-center gap-2 text-sm">
                <input type="checkbox" className="rounded" defaultChecked />
                <span>Min win rate 55%</span>
              </label>
            </div>
          </div>
        </CardContent>
      </Card>

      {/* What-If Calculator */}
      <Card className="bg-primary/5 border-primary/20">
        <CardContent className="p-4">
          <div className="flex flex-wrap items-center gap-4">
            <span className="font-medium">What-If Calculator:</span>
            <span className="text-sm text-muted-foreground">
              If you invested
            </span>
            <div className="flex items-center gap-1">
              <span>$</span>
              <input
                type="number"
                value={whatIfAmount}
                onChange={(e) => setWhatIfAmount(Number(e.target.value))}
                className="w-24 rounded border bg-background px-2 py-1 text-sm"
              />
            </div>
            <span className="text-sm text-muted-foreground">30 days ago...</span>
          </div>
        </CardContent>
      </Card>

      {/* Top Performers */}
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <Star className="h-5 w-5 text-yellow-500" />
          <h2 className="text-xl font-semibold">Top Performers</h2>
        </div>

        <div className="grid gap-4">
          {mockWallets.map((wallet) => {
            const hypotheticalReturn = whatIfAmount * (wallet.roi30d / 100);
            const hypotheticalTotal = whatIfAmount + hypotheticalReturn;
            const equityCurve = walletEquityCurves[wallet.address];

            return (
              <Card key={wallet.address} className="hover:border-primary transition-colors">
                <CardContent className="p-6">
                  <div className="flex flex-col gap-6">
                    {/* Header Row */}
                    <div className="flex flex-col lg:flex-row lg:items-center gap-6">
                      {/* Rank & Address */}
                      <div className="flex items-center gap-4">
                        <div className="flex h-10 w-10 items-center justify-center rounded-full bg-primary text-primary-foreground font-bold">
                          #{wallet.rank}
                        </div>
                        <div>
                          <p className="font-medium font-mono">
                            {shortenAddress(wallet.address)}
                          </p>
                          <span
                            className={`text-xs px-2 py-0.5 rounded-full ${
                              predictionColors[wallet.prediction]
                            }`}
                          >
                            {predictionLabels[wallet.prediction]} ({wallet.confidence}%)
                          </span>
                        </div>
                      </div>

                      {/* Metrics */}
                      <div className="grid grid-cols-2 sm:grid-cols-5 gap-4 flex-1">
                        <div>
                          <p className="text-xs text-muted-foreground">ROI (30d)</p>
                          <p className="font-medium text-profit">
                            +{wallet.roi30d}%
                          </p>
                        </div>
                        <div>
                          <p className="text-xs text-muted-foreground">Sharpe</p>
                          <p className="font-medium">{wallet.sharpe}</p>
                        </div>
                        <div>
                          <p className="text-xs text-muted-foreground">Trades</p>
                          <p className="font-medium">{wallet.trades}</p>
                        </div>
                        <div>
                          <p className="text-xs text-muted-foreground">Win Rate</p>
                          <p className="font-medium">{wallet.winRate}%</p>
                        </div>
                        <div>
                          <p className="text-xs text-muted-foreground">Max DD</p>
                          <p className="font-medium text-loss">
                            {wallet.maxDrawdown}%
                          </p>
                        </div>
                      </div>

                      {/* What-If & Actions */}
                      <div className="flex flex-col sm:flex-row items-start sm:items-center gap-4">
                        <div className="text-sm">
                          <p className="text-muted-foreground">
                            If invested ${whatIfAmount}:
                          </p>
                          <p className="font-medium text-profit text-lg">
                            ${hypotheticalTotal.toFixed(2)}
                          </p>
                        </div>
                        <Button
                          variant={wallet.tracked ? 'outline' : 'default'}
                          size="sm"
                        >
                          {wallet.tracked ? (
                            'Tracking'
                          ) : (
                            <>
                              <Plus className="mr-1 h-4 w-4" />
                              Track
                            </>
                          )}
                        </Button>
                      </div>
                    </div>

                    {/* Equity Curve */}
                    <div className="border rounded-lg p-2 bg-muted/20">
                      <EquityCurve data={equityCurve} height={80} />
                    </div>
                  </div>
                </CardContent>
              </Card>
            );
          })}
        </div>
      </div>
    </div>
  );
}
