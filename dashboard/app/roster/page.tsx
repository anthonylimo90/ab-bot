'use client';

import Link from 'next/link';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { useRosterStore, RosterWallet } from '@/stores/roster-store';
import { useToastStore } from '@/stores/toast-store';
import { useRosterWallets } from '@/hooks/queries';
import { shortenAddress } from '@/lib/utils';
import {
  Users,
  Plus,
  TrendingUp,
  TrendingDown,
  ArrowRight,
  Settings,
  ChevronDown,
  AlertTriangle,
  RefreshCw,
  AlertCircle,
} from 'lucide-react';

const copyBehaviorLabels = {
  copy_all: 'All Trades',
  events_only: 'Events Only',
  arb_threshold: 'Arb Threshold',
};

function EmptySlotCard({ slotNumber }: { slotNumber: number }) {
  return (
    <Card className="border-dashed">
      <CardContent className="p-6 flex flex-col items-center justify-center min-h-[200px] text-center">
        <div className="h-12 w-12 rounded-full bg-muted flex items-center justify-center mb-4">
          <Plus className="h-6 w-6 text-muted-foreground" />
        </div>
        <p className="font-medium text-muted-foreground mb-2">Slot {slotNumber} Available</p>
        <p className="text-sm text-muted-foreground mb-4">
          Add a wallet from the Bench to start copying
        </p>
        <Link href="/bench">
          <Button variant="outline" size="sm">
            Browse Bench
            <ArrowRight className="ml-2 h-4 w-4" />
          </Button>
        </Link>
      </CardContent>
    </Card>
  );
}

function RosterCard({ wallet }: { wallet: RosterWallet }) {
  const toast = useToastStore();
  const { demoteToBench } = useRosterStore();

  const handleDemote = () => {
    demoteToBench(wallet.address);
    toast.info(
      'Moved to Bench',
      `${wallet.label || shortenAddress(wallet.address)} has been demoted to the Bench`
    );
  };

  return (
    <Card className="hover:border-primary transition-colors">
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="h-10 w-10 rounded-full bg-primary flex items-center justify-center text-primary-foreground font-bold">
              {wallet.label?.charAt(0) || wallet.address.charAt(2).toUpperCase()}
            </div>
            <div>
              <CardTitle className="text-base">
                {wallet.label || shortenAddress(wallet.address)}
              </CardTitle>
              <p className="text-xs text-muted-foreground font-mono">
                {shortenAddress(wallet.address)}
              </p>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <span className="text-xs px-2 py-1 rounded-full bg-primary/10 text-primary">
              {wallet.copySettings ? copyBehaviorLabels[wallet.copySettings.copy_behavior] : 'All Trades'}
            </span>
            <Button variant="ghost" size="icon" className="h-8 w-8">
              <Settings className="h-4 w-4" />
            </Button>
          </div>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {/* Metrics Row */}
        <div className="grid grid-cols-4 gap-4 text-sm">
          <div>
            <p className="text-xs text-muted-foreground">ROI (30d)</p>
            <p className={`font-medium ${wallet.roi30d >= 0 ? 'text-profit' : 'text-loss'}`}>
              {wallet.roi30d >= 0 ? '+' : ''}{Number(wallet.roi30d).toFixed(1)}%
            </p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Sharpe</p>
            <p className="font-medium">{Number(wallet.sharpe).toFixed(2)}</p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Win Rate</p>
            <p className="font-medium">{Number(wallet.winRate).toFixed(1)}%</p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Max DD</p>
            <p className="font-medium text-loss">{Number(wallet.maxDrawdown).toFixed(1)}%</p>
          </div>
        </div>

        {/* Allocation Bar */}
        <div className="space-y-1">
          <div className="flex items-center justify-between text-xs">
            <span className="text-muted-foreground">Allocation</span>
            <span className="font-medium">{wallet.copySettings?.allocation_pct || 0}%</span>
          </div>
          <div className="w-full bg-muted rounded-full h-2">
            <div
              className="bg-primary h-2 rounded-full transition-all"
              style={{ width: `${wallet.copySettings?.allocation_pct || 0}%` }}
            />
          </div>
        </div>

        {/* Actions */}
        <div className="flex items-center justify-between pt-2">
          <Link href={`/wallet/${wallet.address}`}>
            <Button variant="outline" size="sm">
              View Details
              <ArrowRight className="ml-2 h-4 w-4" />
            </Button>
          </Link>
          <Button variant="ghost" size="sm" onClick={handleDemote}>
            <ChevronDown className="mr-1 h-4 w-4" />
            Demote
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

export default function RosterPage() {
  const { activeWallets: storeWallets, demoteToBench } = useRosterStore();
  const toast = useToastStore();

  // Fetch active wallets from API
  const { rosterWallets: apiWallets, isLoading, error, refetch } = useRosterWallets();

  // Transform API wallets to RosterWallet format for display
  const apiRosterWallets: RosterWallet[] = (apiWallets || []).map((w) => ({
    address: w.address,
    label: w.label,
    tier: 'active' as const,
    copySettings: {
      copy_behavior: 'copy_all' as const,
      allocation_pct: w.allocation_pct,
      max_position_size: w.max_position_size,
    },
    roi30d: 0, // Will be enriched from metrics
    sharpe: 0,
    winRate: w.win_rate,
    trades: w.total_trades,
    maxDrawdown: 0,
    confidence: Math.round(w.success_score * 100),
    addedAt: w.added_at,
    lastActivity: w.last_activity,
  }));

  // Merge API data with store data (store takes priority for local changes)
  const displayWallets = storeWallets.length > 0 ? storeWallets : apiRosterWallets;
  const emptySlots = 5 - displayWallets.length;

  const totalAllocation = displayWallets.reduce(
    (sum, w) => sum + (w.copySettings?.allocation_pct || 0),
    0
  );

  return (
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight flex items-center gap-2">
            <Users className="h-8 w-8" />
            Active
          </h1>
          <p className="text-muted-foreground">
            Your top-tier wallets being actively copied
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" onClick={() => refetch()} disabled={isLoading}>
            <RefreshCw className={`mr-2 h-4 w-4 ${isLoading ? 'animate-spin' : ''}`} />
            Refresh
          </Button>
          <Link href="/bench">
            <Button>
              <Plus className="mr-2 h-4 w-4" />
              Add from Bench
            </Button>
          </Link>
        </div>
      </div>

      {/* Error State */}
      {error && (
        <Card className="border-destructive">
          <CardContent className="p-6 text-center">
            <AlertCircle className="h-8 w-8 mx-auto mb-2 text-destructive" />
            <p className="text-destructive font-medium">Failed to load roster</p>
            <p className="text-sm text-muted-foreground mt-1">
              {error instanceof Error ? error.message : 'Please try again'}
            </p>
            <Button variant="outline" size="sm" className="mt-4" onClick={() => refetch()}>
              Retry
            </Button>
          </CardContent>
        </Card>
      )}

      {/* Loading State */}
      {isLoading && !error && (
        <div className="space-y-4">
          <div className="grid gap-4 md:grid-cols-4">
            {[1, 2, 3, 4].map((i) => (
              <Card key={i}>
                <CardContent className="p-4 flex items-center gap-3">
                  <Skeleton className="h-10 w-10 rounded-full" />
                  <div className="space-y-2">
                    <Skeleton className="h-4 w-24" />
                    <Skeleton className="h-6 w-16" />
                  </div>
                </CardContent>
              </Card>
            ))}
          </div>
          <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
            {[1, 2, 3].map((i) => (
              <Card key={i}>
                <CardContent className="p-6">
                  <div className="space-y-4">
                    <div className="flex items-center gap-3">
                      <Skeleton className="h-10 w-10 rounded-full" />
                      <div className="space-y-2">
                        <Skeleton className="h-4 w-24" />
                        <Skeleton className="h-3 w-32" />
                      </div>
                    </div>
                    <div className="grid grid-cols-4 gap-2">
                      {[1, 2, 3, 4].map((j) => (
                        <Skeleton key={j} className="h-10 w-full" />
                      ))}
                    </div>
                  </div>
                </CardContent>
              </Card>
            ))}
          </div>
        </div>
      )}

      {/* Summary Stats */}
      {!isLoading && !error && (
        <div className="grid gap-4 md:grid-cols-4">
          <Card>
            <CardContent className="p-4">
              <div className="flex items-center gap-3">
                <div className="h-10 w-10 rounded-full bg-primary/10 flex items-center justify-center">
                  <Users className="h-5 w-5 text-primary" />
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Active Wallets</p>
                  <p className="text-2xl font-bold">{displayWallets.length}/5</p>
                </div>
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardContent className="p-4">
              <div className="flex items-center gap-3">
                <div className="h-10 w-10 rounded-full bg-profit/10 flex items-center justify-center">
                  <TrendingUp className="h-5 w-5 text-profit" />
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Avg. ROI (30d)</p>
                  <p className="text-2xl font-bold text-profit">
                    +{(displayWallets.length > 0 ? displayWallets.reduce((sum, w) => sum + (Number(w.roi30d) || 0), 0) / displayWallets.length : 0).toFixed(1)}%
                  </p>
                </div>
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardContent className="p-4">
              <div className="flex items-center gap-3">
                <div className="h-10 w-10 rounded-full bg-demo/10 flex items-center justify-center">
                  <TrendingDown className="h-5 w-5 text-demo" />
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Total Allocation</p>
                  <p className="text-2xl font-bold">{totalAllocation}%</p>
                </div>
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardContent className="p-4">
              <div className="flex items-center gap-3">
                <div className="h-10 w-10 rounded-full bg-yellow-500/10 flex items-center justify-center">
                  <AlertTriangle className="h-5 w-5 text-yellow-500" />
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Avg. Max DD</p>
                  <p className="text-2xl font-bold text-loss">
                    {(displayWallets.length > 0 ? displayWallets.reduce((sum, w) => sum + (Number(w.maxDrawdown) || 0), 0) / displayWallets.length : 0).toFixed(1)}%
                  </p>
                </div>
              </div>
            </CardContent>
          </Card>
        </div>
      )}

      {/* Roster Grid */}
      {!isLoading && !error && (
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {displayWallets.map((wallet) => (
            <RosterCard key={wallet.address} wallet={wallet} />
          ))}
          {Array.from({ length: emptySlots }).map((_, i) => (
            <EmptySlotCard key={`empty-${i}`} slotNumber={displayWallets.length + i + 1} />
          ))}
        </div>
      )}
    </div>
  );
}
