'use client';

import { useState, useMemo } from 'react';
import { Card, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { EquityCurve } from '@/components/charts/EquityCurve';
import { CopyWalletModal } from '@/components/modals/CopyWalletModal';
import { useToastStore } from '@/stores/toast-store';
import { formatCurrency, shortenAddress } from '@/lib/utils';
import { Star, Plus, Check, Loader2 } from 'lucide-react';
import type { CopyBehavior } from '@/types/api';

// Generate mock equity curve data
function generateEquityCurve(days: number, roi: number) {
  const data: { time: string; value: number }[] = [];
  let value = 100;
  const dailyReturn = Math.pow(1 + roi / 100, 1 / days) - 1;
  const now = new Date();

  for (let i = days; i >= 0; i--) {
    const date = new Date(now);
    date.setDate(date.getDate() - i);
    const randomFactor = 1 + (Math.random() - 0.5) * 0.04;
    value = value * (1 + dailyReturn) * randomFactor;
    data.push({
      time: date.toISOString().split('T')[0],
      value: Math.round(value * 100) / 100,
    });
  }
  return data;
}

// Mock data - extended with more properties
const initialMockWallets = [
  {
    address: '0x1234567890abcdef1234567890abcdef12345678',
    rank: 1,
    roi30d: 47.3,
    roi7d: 12.1,
    roi90d: 89.2,
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
    roi7d: 8.4,
    roi90d: 72.5,
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
    roi7d: 5.2,
    roi90d: 54.8,
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
    roi7d: 3.1,
    roi90d: 41.2,
    sharpe: 1.3,
    trades: 78,
    winRate: 61,
    maxDrawdown: -18.5,
    prediction: 'LOW_POTENTIAL' as const,
    confidence: 52,
    tracked: false,
  },
  {
    address: '0xfedcba9876543210fedcba9876543210fedcba98',
    rank: 5,
    roi30d: 18.5,
    roi7d: 2.8,
    roi90d: 35.1,
    sharpe: 1.1,
    trades: 45,
    winRate: 53,
    maxDrawdown: -22.1,
    prediction: 'LOW_POTENTIAL' as const,
    confidence: 45,
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

type SortField = 'roi' | 'sharpe' | 'winRate' | 'trades';
type TimePeriod = '7d' | '30d' | '90d';

export default function DiscoverPage() {
  const toast = useToastStore();

  // Filter state
  const [sortBy, setSortBy] = useState<SortField>('roi');
  const [timePeriod, setTimePeriod] = useState<TimePeriod>('30d');
  const [minTrades, setMinTrades] = useState<string>('10');
  const [hideBots, setHideBots] = useState(true);
  const [minWinRate, setMinWinRate] = useState(true);

  // What-if calculator
  const [whatIfAmount, setWhatIfAmount] = useState(100);

  // Track button loading states
  const [trackingLoading, setTrackingLoading] = useState<Record<string, boolean>>({});

  // Wallet tracking state (simulated - would come from store in real app)
  const [wallets, setWallets] = useState(initialMockWallets);

  // Copy modal state
  const [copyModalOpen, setCopyModalOpen] = useState(false);
  const [selectedWallet, setSelectedWallet] = useState<typeof wallets[0] | null>(null);
  const [rosterCount] = useState(2);

  // Generate equity curves (memoized)
  const walletEquityCurves = useMemo(() => {
    return wallets.reduce((acc, wallet) => {
      const roi = timePeriod === '7d' ? wallet.roi7d : timePeriod === '90d' ? wallet.roi90d : wallet.roi30d;
      const days = timePeriod === '7d' ? 7 : timePeriod === '90d' ? 90 : 30;
      acc[wallet.address] = generateEquityCurve(days, roi);
      return acc;
    }, {} as Record<string, { time: string; value: number }[]>);
  }, [wallets, timePeriod]);

  // Filter and sort wallets
  const filteredWallets = useMemo(() => {
    let result = [...wallets];

    // Apply min trades filter
    const minTradesNum = parseInt(minTrades);
    if (!isNaN(minTradesNum)) {
      result = result.filter(w => w.trades >= minTradesNum);
    }

    // Apply min win rate filter
    if (minWinRate) {
      result = result.filter(w => w.winRate >= 55);
    }

    // Sort
    result.sort((a, b) => {
      switch (sortBy) {
        case 'roi':
          const roiA = timePeriod === '7d' ? a.roi7d : timePeriod === '90d' ? a.roi90d : a.roi30d;
          const roiB = timePeriod === '7d' ? b.roi7d : timePeriod === '90d' ? b.roi90d : b.roi30d;
          return roiB - roiA;
        case 'sharpe':
          return b.sharpe - a.sharpe;
        case 'winRate':
          return b.winRate - a.winRate;
        case 'trades':
          return b.trades - a.trades;
        default:
          return 0;
      }
    });

    // Re-rank after filtering
    return result.map((w, i) => ({ ...w, rank: i + 1 }));
  }, [wallets, sortBy, timePeriod, minTrades, minWinRate]);

  const handleTrack = async (address: string) => {
    setTrackingLoading(prev => ({ ...prev, [address]: true }));

    // Simulate API call
    await new Promise(resolve => setTimeout(resolve, 800));

    setWallets(prev => prev.map(w =>
      w.address === address ? { ...w, tracked: !w.tracked } : w
    ));

    const wallet = wallets.find(w => w.address === address);
    const wasTracked = wallet?.tracked;

    if (wasTracked) {
      toast.info('Removed from Bench', `${shortenAddress(address)} is no longer being monitored`);
    } else {
      toast.success('Added to Bench', `${shortenAddress(address)} is now being monitored`);
    }

    setTrackingLoading(prev => ({ ...prev, [address]: false }));
  };

  const handleCopyClick = (wallet: typeof wallets[0]) => {
    setSelectedWallet(wallet);
    setCopyModalOpen(true);
  };

  const handleCopyConfirm = (settings: {
    address: string;
    allocation_pct: number;
    copy_behavior: CopyBehavior;
    max_position_size: number;
    tier: 'active' | 'bench';
  }) => {
    const tierLabel = settings.tier === 'active' ? 'Active 5' : 'Bench';
    toast.success(
      `Wallet added to ${tierLabel}`,
      `${shortenAddress(settings.address)} is now being ${settings.tier === 'active' ? 'copied' : 'monitored'}`
    );

    // Mark as tracked
    setWallets(prev => prev.map(w =>
      w.address === settings.address ? { ...w, tracked: true } : w
    ));

    setCopyModalOpen(false);
    setSelectedWallet(null);
  };

  const getROI = (wallet: typeof wallets[0]) => {
    return timePeriod === '7d' ? wallet.roi7d : timePeriod === '90d' ? wallet.roi90d : wallet.roi30d;
  };

  const timePeriodLabel = timePeriod === '7d' ? '7 Days' : timePeriod === '90d' ? '90 Days' : '30 Days';

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
              <Select value={sortBy} onValueChange={(v) => setSortBy(v as SortField)}>
                <SelectTrigger className="w-[120px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="roi">ROI</SelectItem>
                  <SelectItem value="sharpe">Sharpe</SelectItem>
                  <SelectItem value="winRate">Win Rate</SelectItem>
                  <SelectItem value="trades">Trades</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">Time:</span>
              <Select value={timePeriod} onValueChange={(v) => setTimePeriod(v as TimePeriod)}>
                <SelectTrigger className="w-[110px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="7d">7 Days</SelectItem>
                  <SelectItem value="30d">30 Days</SelectItem>
                  <SelectItem value="90d">90 Days</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">Min Trades:</span>
              <Select value={minTrades} onValueChange={setMinTrades}>
                <SelectTrigger className="w-[80px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="0">Any</SelectItem>
                  <SelectItem value="10">10+</SelectItem>
                  <SelectItem value="50">50+</SelectItem>
                  <SelectItem value="100">100+</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="ml-auto flex items-center gap-4">
              <label className="flex items-center gap-2 text-sm cursor-pointer">
                <Switch
                  checked={hideBots}
                  onCheckedChange={setHideBots}
                />
                <span>Hide bots</span>
              </label>
              <label className="flex items-center gap-2 text-sm cursor-pointer">
                <Switch
                  checked={minWinRate}
                  onCheckedChange={setMinWinRate}
                />
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
            <span className="text-sm text-muted-foreground">{timePeriodLabel.toLowerCase()} ago...</span>
          </div>
        </CardContent>
      </Card>

      {/* Top Performers */}
      <div className="space-y-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Star className="h-5 w-5 text-yellow-500" />
            <h2 className="text-xl font-semibold">Top Performers</h2>
          </div>
          <span className="text-sm text-muted-foreground">
            {filteredWallets.length} wallets
          </span>
        </div>

        <div className="grid gap-4">
          {filteredWallets.map((wallet) => {
            const roi = getROI(wallet);
            const hypotheticalReturn = whatIfAmount * (roi / 100);
            const hypotheticalTotal = whatIfAmount + hypotheticalReturn;
            const equityCurve = walletEquityCurves[wallet.address];
            const isLoading = trackingLoading[wallet.address];

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
                          <p className="text-xs text-muted-foreground">ROI ({timePeriod})</p>
                          <p className={`font-medium ${roi >= 0 ? 'text-profit' : 'text-loss'}`}>
                            {roi >= 0 ? '+' : ''}{roi}%
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
                          <p className={`font-medium text-lg ${hypotheticalReturn >= 0 ? 'text-profit' : 'text-loss'}`}>
                            ${hypotheticalTotal.toFixed(2)}
                          </p>
                        </div>
                        <div className="flex gap-2">
                          <Button
                            variant={wallet.tracked ? 'outline' : 'secondary'}
                            size="sm"
                            onClick={() => handleTrack(wallet.address)}
                            disabled={isLoading}
                          >
                            {isLoading ? (
                              <Loader2 className="h-4 w-4 animate-spin" />
                            ) : wallet.tracked ? (
                              <>
                                <Check className="mr-1 h-4 w-4" />
                                Tracking
                              </>
                            ) : (
                              <>
                                <Plus className="mr-1 h-4 w-4" />
                                Track
                              </>
                            )}
                          </Button>
                          <Button
                            variant="default"
                            size="sm"
                            onClick={() => handleCopyClick(wallet)}
                          >
                            Copy
                          </Button>
                        </div>
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

          {filteredWallets.length === 0 && (
            <Card>
              <CardContent className="p-12 text-center">
                <p className="text-muted-foreground">
                  No wallets match your filters. Try adjusting the criteria.
                </p>
              </CardContent>
            </Card>
          )}
        </div>
      </div>

      {/* Copy Wallet Modal */}
      <CopyWalletModal
        wallet={selectedWallet ? {
          address: selectedWallet.address,
          roi30d: selectedWallet.roi30d,
          sharpe: selectedWallet.sharpe,
          winRate: selectedWallet.winRate,
          trades: selectedWallet.trades,
          confidence: selectedWallet.confidence,
        } : null}
        isOpen={copyModalOpen}
        onClose={() => {
          setCopyModalOpen(false);
          setSelectedWallet(null);
        }}
        onConfirm={handleCopyConfirm}
        rosterCount={rosterCount}
      />
    </div>
  );
}
