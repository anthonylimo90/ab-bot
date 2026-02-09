'use client';

import { useState, useMemo } from 'react';
import { Card, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { Skeleton } from '@/components/ui/skeleton';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { CopyWalletModal } from '@/components/modals/CopyWalletModal';
import { useDiscoverWalletsQuery } from '@/hooks/queries/useDiscoverQuery';
import {
  useAddAllocationMutation,
  useAllocationsQuery,
  useDemoteAllocationMutation,
  usePromoteAllocationMutation,
  useRemoveAllocationMutation,
  useUpdateAllocationMutation,
} from '@/hooks/queries/useAllocationsQuery';
import { useToastStore } from '@/stores/toast-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { useModeStore } from '@/stores/mode-store';
import { shortenAddress } from '@/lib/utils';
import { Star, Plus, Check, Loader2, Search } from 'lucide-react';
import type { CopyBehavior, DiscoveredWallet, PredictionCategory, WorkspaceAllocation } from '@/types/api';

// Transform API wallet to component format
interface DisplayWallet {
  address: string;
  rank: number;
  roi30d: number;
  roi7d: number;
  roi90d: number;
  sharpe: number;
  trades: number;
  winRate: number;
  maxDrawdown: number;
  prediction: PredictionCategory;
  confidence: number;
  tracked: boolean;
}

function toDisplayWallet(w: DiscoveredWallet, rank: number): DisplayWallet {
  return {
    address: w.address,
    rank,
    roi30d: Number(w.roi_30d) || 0,
    roi7d: Number(w.roi_7d) || 0,
    roi90d: Number(w.roi_90d) || 0,
    sharpe: Number(w.sharpe_ratio) || 0,
    trades: Number(w.total_trades) || 0,
    winRate: Number(w.win_rate) || 0,
    maxDrawdown: Number(w.max_drawdown) || 0,
    prediction: w.prediction,
    confidence: Number(w.confidence) || 0,
    tracked: w.is_tracked,
  };
}

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
  const { currentWorkspace } = useWorkspaceStore();
  const { mode } = useModeStore();
  const { data: allocations = [] } = useAllocationsQuery(currentWorkspace?.id, mode);
  const addAllocationMutation = useAddAllocationMutation(currentWorkspace?.id, mode);
  const removeAllocationMutation = useRemoveAllocationMutation(currentWorkspace?.id, mode);
  const promoteAllocationMutation = usePromoteAllocationMutation(currentWorkspace?.id, mode);
  const demoteAllocationMutation = useDemoteAllocationMutation(currentWorkspace?.id, mode);
  const updateAllocationMutation = useUpdateAllocationMutation(currentWorkspace?.id, mode);

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

  // Copy modal state
  const [copyModalOpen, setCopyModalOpen] = useState(false);
  const [selectedWallet, setSelectedWallet] = useState<DisplayWallet | null>(null);

  // Active allocation count
  const rosterCount = allocations.filter((a) => a.tier === 'active').length;

  // Fetch wallets from API
  const { data: apiWallets, isLoading, error, refetch } = useDiscoverWalletsQuery(mode, {
    sortBy: sortBy,
    period: timePeriod,
    minTrades: minTrades === '0' ? undefined : parseInt(minTrades),
    minWinRate: minWinRate ? 55 : undefined,
    limit: 50,
    workspaceId: currentWorkspace?.id,
  });

  // Check if wallet is tracked in workspace allocations
  const isWalletTracked = (address: string): boolean => {
    const lowerAddress = address.toLowerCase();
    return allocations.some((a) => a.wallet_address.toLowerCase() === lowerAddress);
  };

  const getAllocation = (address: string): WorkspaceAllocation | undefined =>
    allocations.find((a) => a.wallet_address.toLowerCase() === address.toLowerCase());

  // Transform and filter wallets
  const filteredWallets = useMemo(() => {
    if (!apiWallets) return [];

    // Transform API wallets to display format
    let result = apiWallets.map((w, i) => ({
      ...toDisplayWallet(w, i + 1),
      tracked: allocations.some((a) => a.wallet_address.toLowerCase() === w.address.toLowerCase()),
    }));

    // Sort (API already returns sorted, but we can re-sort client-side if needed)
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

    if (hideBots) {
      // Lightweight heuristic until API exposes bot classification.
      result = result.filter((wallet) => !(wallet.trades > 1000 && wallet.roi30d < 2));
    }

    // Re-rank after filtering/sorting
    return result.map((w, i) => ({ ...w, rank: i + 1 }));
  }, [apiWallets, sortBy, timePeriod, allocations, hideBots]);

  const handleTrack = async (address: string) => {
    setTrackingLoading(prev => ({ ...prev, [address]: true }));

    const isTracked = isWalletTracked(address);

    try {
      if (isTracked) {
        await removeAllocationMutation.mutateAsync(address);
        toast.info('Removed from Tracking', `${shortenAddress(address)} is no longer being monitored`);
      } else {
        await addAllocationMutation.mutateAsync({ address, params: { tier: 'bench' } });
        toast.success('Added to Watching', `${shortenAddress(address)} is now being monitored`);
      }
    } catch {
      toast.error('Update Failed', 'Could not update wallet tracking');
    } finally {
      setTrackingLoading(prev => ({ ...prev, [address]: false }));
    }
  };

  const handleCopyClick = (wallet: DisplayWallet) => {
    setSelectedWallet(wallet);
    setCopyModalOpen(true);
  };

  const handleCopyConfirm = async (settings: {
    address: string;
    allocation_pct: number;
    copy_behavior: CopyBehavior;
    max_position_size: number;
    tier: 'active' | 'bench';
  }) => {
    try {
      const existing = getAllocation(settings.address);
      if (!existing) {
        await addAllocationMutation.mutateAsync({
          address: settings.address,
          params: {
            tier: settings.tier,
            allocation_pct: settings.allocation_pct,
            copy_behavior: settings.copy_behavior,
            max_position_size: settings.max_position_size,
          },
        });
      } else {
        await updateAllocationMutation.mutateAsync({
          address: settings.address,
          params: {
            allocation_pct: settings.allocation_pct,
            copy_behavior: settings.copy_behavior,
            max_position_size: settings.max_position_size,
          },
        });
        if (settings.tier === 'active' && existing.tier !== 'active') {
          await promoteAllocationMutation.mutateAsync(settings.address);
        }
        if (settings.tier === 'bench' && existing.tier !== 'bench') {
          await demoteAllocationMutation.mutateAsync(settings.address);
        }
      }

      const tierLabel = settings.tier === 'active' ? 'Active' : 'Watching';
      toast.success(
        `Wallet added to ${tierLabel}`,
        `${shortenAddress(settings.address)} is now being ${settings.tier === 'active' ? 'copied' : 'monitored'}`
      );
      setCopyModalOpen(false);
      setSelectedWallet(null);
    } catch {
      toast.error('Copy Setup Failed', 'Could not save wallet copy settings');
    }
  };

  const getROI = (wallet: DisplayWallet) => {
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
            {isLoading ? 'Loading...' : `${filteredWallets.length} wallets`}
          </span>
        </div>

        <div className="grid gap-4">
          {/* Loading State */}
          {isLoading && (
            <>
              {[1, 2, 3].map((i) => (
                <Card key={i}>
                  <CardContent className="p-6">
                    <div className="flex flex-col lg:flex-row lg:items-center gap-6">
                      <div className="flex items-center gap-4">
                        <Skeleton className="h-10 w-10 rounded-full" />
                        <div className="space-y-2">
                          <Skeleton className="h-4 w-32" />
                          <Skeleton className="h-3 w-24" />
                        </div>
                      </div>
                      <div className="grid grid-cols-5 gap-4 flex-1">
                        {[1, 2, 3, 4, 5].map((j) => (
                          <div key={j} className="space-y-2">
                            <Skeleton className="h-3 w-12" />
                            <Skeleton className="h-4 w-16" />
                          </div>
                        ))}
                      </div>
                      <div className="flex gap-2">
                        <Skeleton className="h-8 w-24" />
                        <Skeleton className="h-8 w-16" />
                      </div>
                    </div>
                  </CardContent>
                </Card>
              ))}
            </>
          )}

          {/* Error State */}
          {error && !isLoading && (
            <Card>
              <CardContent className="p-12 text-center">
                <p className="text-destructive mb-4">
                  Failed to load wallets. Please try again.
                </p>
                <Button variant="outline" onClick={() => refetch()}>
                  Retry
                </Button>
              </CardContent>
            </Card>
          )}

          {/* Wallet List */}
          {!isLoading && !error && filteredWallets.map((wallet) => {
            const roi = getROI(wallet);
            const hypotheticalReturn = whatIfAmount * (roi / 100);
            const hypotheticalTotal = whatIfAmount + hypotheticalReturn;
            const isTrackLoading = trackingLoading[wallet.address];

            return (
              <Card key={wallet.address} className="hover:border-primary transition-colors">
                <CardContent className="p-6">
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
                          {roi >= 0 ? '+' : ''}{roi.toFixed(1)}%
                        </p>
                      </div>
                      <div>
                        <p className="text-xs text-muted-foreground">Sharpe</p>
                        <p className="font-medium">{wallet.sharpe.toFixed(2)}</p>
                      </div>
                      <div>
                        <p className="text-xs text-muted-foreground">Trades</p>
                        <p className="font-medium">{wallet.trades}</p>
                      </div>
                      <div>
                        <p className="text-xs text-muted-foreground">Win Rate</p>
                        <p className="font-medium">{wallet.winRate.toFixed(1)}%</p>
                      </div>
                      <div>
                        <p className="text-xs text-muted-foreground">Max DD</p>
                        <p className="font-medium text-loss">
                          {wallet.maxDrawdown.toFixed(1)}%
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
                          disabled={isTrackLoading}
                        >
                          {isTrackLoading ? (
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
                </CardContent>
              </Card>
            );
          })}

          {/* Empty State */}
          {!isLoading && !error && filteredWallets.length === 0 && (
            <Card>
              <CardContent className="p-12 text-center">
                <Search className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
                <h3 className="text-lg font-medium mb-2">No wallets found</h3>
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
