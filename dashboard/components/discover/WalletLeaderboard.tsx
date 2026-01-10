'use client';

import { useEffect, useState } from 'react';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { cn, formatPercent, formatCurrency, shortenAddress } from '@/lib/utils';
import type { DiscoveredWallet, PredictionCategory } from '@/types/api';
import api from '@/lib/api';

interface WalletLeaderboardProps {
  className?: string;
  onTrackWallet?: (address: string) => void;
}

type SortBy = 'roi' | 'sharpe' | 'winRate' | 'trades';
type Period = '7d' | '30d' | '90d';

export function WalletLeaderboard({
  className,
  onTrackWallet,
}: WalletLeaderboardProps) {
  const [wallets, setWallets] = useState<DiscoveredWallet[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [sortBy, setSortBy] = useState<SortBy>('roi');
  const [period, setPeriod] = useState<Period>('30d');

  const fetchWallets = async () => {
    setIsLoading(true);
    try {
      const data = await api.discoverWallets({ sort_by: sortBy, period, limit: 10 });
      setWallets(data);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch wallets');
    } finally {
      setIsLoading(false);
    }
  };

  useEffect(() => {
    fetchWallets();
  }, [sortBy, period]);

  const sortOptions: { value: SortBy; label: string }[] = [
    { value: 'roi', label: 'ROI' },
    { value: 'sharpe', label: 'Sharpe' },
    { value: 'winRate', label: 'Win Rate' },
    { value: 'trades', label: 'Activity' },
  ];

  const periodOptions: { value: Period; label: string }[] = [
    { value: '7d', label: '7D' },
    { value: '30d', label: '30D' },
    { value: '90d', label: '90D' },
  ];

  return (
    <Card className={cn('', className)}>
      <CardHeader>
        <div className="flex items-center justify-between">
          <div>
            <CardTitle className="text-lg">Top Wallets</CardTitle>
            <CardDescription>Best-performing wallets to copy trade</CardDescription>
          </div>
          <div className="flex items-center gap-2">
            {periodOptions.map(({ value, label }) => (
              <Button
                key={value}
                variant={period === value ? 'secondary' : 'ghost'}
                size="sm"
                className="h-7 px-2 text-xs"
                onClick={() => setPeriod(value)}
              >
                {label}
              </Button>
            ))}
          </div>
        </div>
        <div className="flex gap-1 pt-2">
          {sortOptions.map(({ value, label }) => (
            <Button
              key={value}
              variant={sortBy === value ? 'default' : 'outline'}
              size="sm"
              className="h-7"
              onClick={() => setSortBy(value)}
            >
              {label}
            </Button>
          ))}
        </div>
      </CardHeader>
      <CardContent className="p-0">
        {isLoading ? (
          <div className="flex items-center justify-center py-8">
            <div className="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent" />
          </div>
        ) : error ? (
          <div className="px-6 py-4 text-center text-sm text-muted-foreground">
            {error}
          </div>
        ) : wallets.length === 0 ? (
          <div className="px-6 py-4 text-center text-sm text-muted-foreground">
            No wallets found
          </div>
        ) : (
          <div className="divide-y divide-border">
            {wallets.map((wallet) => (
              <WalletRow
                key={wallet.address}
                wallet={wallet}
                period={period}
                onTrack={onTrackWallet}
              />
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function WalletRow({
  wallet,
  period,
  onTrack,
}: {
  wallet: DiscoveredWallet;
  period: Period;
  onTrack?: (address: string) => void;
}) {
  const roi = period === '7d' ? wallet.roi_7d : period === '90d' ? wallet.roi_90d : wallet.roi_30d;

  return (
    <div className="flex items-center gap-3 px-4 py-3 hover:bg-muted/50 transition-colors">
      {/* Rank */}
      <div
        className={cn(
          'flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-xs font-bold',
          wallet.rank === 1 && 'bg-yellow-500/20 text-yellow-500',
          wallet.rank === 2 && 'bg-slate-400/20 text-slate-400',
          wallet.rank === 3 && 'bg-amber-600/20 text-amber-600',
          wallet.rank > 3 && 'bg-muted text-muted-foreground'
        )}
      >
        {wallet.rank}
      </div>

      {/* Address & Prediction */}
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="font-mono text-sm">{shortenAddress(wallet.address)}</span>
          <PredictionBadge category={wallet.prediction} confidence={wallet.confidence} />
        </div>
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <span>{wallet.total_trades} trades</span>
          <span>·</span>
          <span>{formatPercent(wallet.win_rate)} win</span>
          <span>·</span>
          <span>{wallet.trades_24h} today</span>
        </div>
      </div>

      {/* ROI & Actions */}
      <div className="flex items-center gap-3 shrink-0">
        <div className="text-right">
          <div
            className={cn(
              'text-sm font-semibold',
              roi >= 0 ? 'text-profit' : 'text-loss'
            )}
          >
            {formatPercent(roi, { showSign: true })}
          </div>
          <div className="text-xs text-muted-foreground">
            Sharpe: {wallet.sharpe_ratio.toFixed(2)}
          </div>
        </div>
        {onTrack && !wallet.is_tracked && (
          <Button
            size="sm"
            variant="outline"
            className="h-7 text-xs"
            onClick={() => onTrack(wallet.address)}
          >
            Track
          </Button>
        )}
        {wallet.is_tracked && (
          <span className="text-xs text-profit font-medium">Tracking</span>
        )}
      </div>
    </div>
  );
}

function PredictionBadge({
  category,
  confidence,
}: {
  category: PredictionCategory;
  confidence: number;
}) {
  const config: Record<PredictionCategory, { label: string; className: string }> = {
    HIGH_POTENTIAL: { label: 'High', className: 'bg-profit/10 text-profit' },
    MODERATE: { label: 'Moderate', className: 'bg-yellow-500/10 text-yellow-500' },
    LOW_POTENTIAL: { label: 'Low', className: 'bg-muted text-muted-foreground' },
    INSUFFICIENT_DATA: { label: 'N/A', className: 'bg-muted text-muted-foreground' },
  };

  const { label, className } = config[category] || config.INSUFFICIENT_DATA;

  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-xs font-medium',
        className
      )}
    >
      {label}
      <span className="opacity-60">{confidence}%</span>
    </span>
  );
}

export default WalletLeaderboard;
