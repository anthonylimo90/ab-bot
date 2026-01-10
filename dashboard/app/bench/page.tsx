'use client';

import { useState, useMemo } from 'react';
import Link from 'next/link';
import { Card, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Skeleton } from '@/components/ui/skeleton';
import { EquityCurve } from '@/components/charts/EquityCurve';
import { CopyWalletModal } from '@/components/modals/CopyWalletModal';
import { useRosterStore, createRosterWallet } from '@/stores/roster-store';
import { useToastStore } from '@/stores/toast-store';
import { useDiscoverWalletsQuery } from '@/hooks/queries/useDiscoverQuery';
import { shortenAddress } from '@/lib/utils';
import {
  UserCheck,
  Search,
  Plus,
  ChevronUp,
  Loader2,
  Star,
  Trash2,
  ArrowRight,
  RefreshCw,
  AlertCircle,
} from 'lucide-react';
import type { CopyBehavior, DiscoveredWallet, PredictionCategory } from '@/types/api';

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

const predictionColors: Record<PredictionCategory, string> = {
  HIGH_POTENTIAL: 'text-profit bg-profit/10',
  MODERATE: 'text-yellow-500 bg-yellow-500/10',
  LOW_POTENTIAL: 'text-loss bg-loss/10',
  INSUFFICIENT_DATA: 'text-muted-foreground bg-muted',
};

const predictionLabels: Record<PredictionCategory, string> = {
  HIGH_POTENTIAL: 'High Potential',
  MODERATE: 'Moderate',
  LOW_POTENTIAL: 'Low Potential',
  INSUFFICIENT_DATA: 'Insufficient Data',
};

type SortField = 'roi' | 'sharpe' | 'winRate' | 'trades';

export default function BenchPage() {
  const toast = useToastStore();
  const {
    activeWallets,
    benchWallets,
    addToBench,
    removeFromBench,
    promoteToActive,
    isRosterFull,
  } = useRosterStore();

  const [activeTab, setActiveTab] = useState('tracked');
  const [sortBy, setSortBy] = useState<SortField>('roi');
  const [minWinRate, setMinWinRate] = useState(true);
  const [loadingStates, setLoadingStates] = useState<Record<string, boolean>>({});

  // Modal state
  const [copyModalOpen, setCopyModalOpen] = useState(false);
  const [selectedWallet, setSelectedWallet] = useState<DiscoveredWallet | null>(null);

  // Fetch discovered wallets from API
  const {
    data: discoveredWallets = [],
    isLoading: isLoadingDiscover,
    error: discoverError,
    refetch: refetchDiscover,
  } = useDiscoverWalletsQuery({
    sortBy,
    period: '30d',
    minWinRate: minWinRate ? 55 : undefined,
    limit: 20,
  });

  // Equity curves
  const equityCurves = useMemo(() => {
    const all = [...benchWallets, ...discoveredWallets];
    return all.reduce((acc, w) => {
      const roi = 'roi30d' in w ? w.roi30d : w.roi_30d;
      acc[w.address] = generateEquityCurve(30, roi);
      return acc;
    }, {} as Record<string, { time: string; value: number }[]>);
  }, [benchWallets, discoveredWallets]);

  // Filter discoverable wallets (exclude already tracked)
  const filteredDiscoverable = useMemo(() => {
    const trackedAddresses = new Set([
      ...activeWallets.map((w) => w.address),
      ...benchWallets.map((w) => w.address),
    ]);

    return discoveredWallets.filter(
      (w) => !trackedAddresses.has(w.address)
    );
  }, [discoveredWallets, activeWallets, benchWallets]);

  const handleAddToBench = async (wallet: DiscoveredWallet) => {
    setLoadingStates((prev) => ({ ...prev, [wallet.address]: true }));
    await new Promise((r) => setTimeout(r, 500));

    const rosterWallet = createRosterWallet(wallet.address, {
      roi30d: wallet.roi_30d,
      sharpe: wallet.sharpe_ratio,
      winRate: wallet.win_rate,
      trades: wallet.total_trades,
      maxDrawdown: wallet.max_drawdown,
      confidence: wallet.confidence,
    });

    addToBench(rosterWallet);
    toast.success('Added to Bench', `${shortenAddress(wallet.address)} is now being monitored`);
    setLoadingStates((prev) => ({ ...prev, [wallet.address]: false }));
  };

  const handlePromote = async (address: string) => {
    if (isRosterFull()) {
      toast.error('Roster Full', 'Demote a wallet from Active 5 first');
      return;
    }

    setLoadingStates((prev) => ({ ...prev, [address]: true }));
    await new Promise((r) => setTimeout(r, 500));

    promoteToActive(address);
    toast.success('Promoted!', `${shortenAddress(address)} added to Active 5`);
    setLoadingStates((prev) => ({ ...prev, [address]: false }));
  };

  const handleRemove = (address: string) => {
    removeFromBench(address);
    toast.info('Removed', `${shortenAddress(address)} removed from Bench`);
  };

  const handleCopyClick = (wallet: DiscoveredWallet) => {
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
    const wallet = selectedWallet;
    if (!wallet) return;

    const rosterWallet = createRosterWallet(
      wallet.address,
      {
        roi30d: wallet.roi_30d,
        sharpe: wallet.sharpe_ratio,
        winRate: wallet.win_rate,
        trades: wallet.total_trades,
        maxDrawdown: wallet.max_drawdown,
        confidence: wallet.confidence,
      },
      settings.tier,
      {
        copy_behavior: settings.copy_behavior,
        allocation_pct: settings.allocation_pct,
        max_position_size: settings.max_position_size,
      }
    );

    if (settings.tier === 'active') {
      const { addToActive } = useRosterStore.getState();
      addToActive(rosterWallet);
    } else {
      addToBench(rosterWallet);
    }

    const tierLabel = settings.tier === 'active' ? 'Active 5' : 'Bench';
    toast.success(`Added to ${tierLabel}`, `${shortenAddress(settings.address)} is now ${settings.tier === 'active' ? 'being copied' : 'being monitored'}`);
    setCopyModalOpen(false);
    setSelectedWallet(null);
  };

  return (
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight flex items-center gap-2">
            <UserCheck className="h-8 w-8" />
            Bench
          </h1>
          <p className="text-muted-foreground">
            Monitor and evaluate wallets before adding to your Active 5
          </p>
        </div>
      </div>

      {/* Tabs */}
      <Tabs value={activeTab} onValueChange={setActiveTab}>
        <TabsList>
          <TabsTrigger value="tracked" className="flex items-center gap-2">
            <UserCheck className="h-4 w-4" />
            Tracked ({benchWallets.length})
          </TabsTrigger>
          <TabsTrigger value="discover" className="flex items-center gap-2">
            <Search className="h-4 w-4" />
            Discover New
          </TabsTrigger>
        </TabsList>

        {/* Tracked Tab */}
        <TabsContent value="tracked" className="space-y-4">
          {benchWallets.length === 0 ? (
            <Card>
              <CardContent className="p-12 text-center">
                <UserCheck className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
                <h3 className="text-lg font-medium mb-2">No wallets on the Bench</h3>
                <p className="text-muted-foreground mb-4">
                  Start by discovering promising wallets to monitor
                </p>
                <Button onClick={() => setActiveTab('discover')}>
                  <Search className="mr-2 h-4 w-4" />
                  Discover Wallets
                </Button>
              </CardContent>
            </Card>
          ) : (
            <div className="grid gap-4">
              {benchWallets.map((wallet) => (
                <Card key={wallet.address} className="hover:border-primary transition-colors">
                  <CardContent className="p-6">
                    <div className="flex flex-col lg:flex-row lg:items-center gap-6">
                      {/* Address & Label */}
                      <div className="flex items-center gap-4 min-w-[200px]">
                        <div className="h-10 w-10 rounded-full bg-muted flex items-center justify-center font-bold">
                          {wallet.label?.charAt(0) || wallet.address.charAt(2).toUpperCase()}
                        </div>
                        <div>
                          <p className="font-medium font-mono">{shortenAddress(wallet.address)}</p>
                          <p className="text-xs text-muted-foreground">
                            Added {new Date(wallet.addedAt).toLocaleDateString()}
                          </p>
                        </div>
                      </div>

                      {/* Metrics */}
                      <div className="grid grid-cols-4 gap-4 flex-1 text-sm">
                        <div>
                          <p className="text-xs text-muted-foreground">ROI (30d)</p>
                          <p className={`font-medium ${wallet.roi30d >= 0 ? 'text-profit' : 'text-loss'}`}>
                            {wallet.roi30d >= 0 ? '+' : ''}{wallet.roi30d}%
                          </p>
                        </div>
                        <div>
                          <p className="text-xs text-muted-foreground">Sharpe</p>
                          <p className="font-medium">{wallet.sharpe}</p>
                        </div>
                        <div>
                          <p className="text-xs text-muted-foreground">Win Rate</p>
                          <p className="font-medium">{wallet.winRate}%</p>
                        </div>
                        <div>
                          <p className="text-xs text-muted-foreground">Trades</p>
                          <p className="font-medium">{wallet.trades}</p>
                        </div>
                      </div>

                      {/* Actions */}
                      <div className="flex items-center gap-2">
                        <Link href={`/wallet/${wallet.address}`}>
                          <Button variant="outline" size="sm">
                            Details
                            <ArrowRight className="ml-1 h-4 w-4" />
                          </Button>
                        </Link>
                        <Button
                          variant="default"
                          size="sm"
                          onClick={() => handlePromote(wallet.address)}
                          disabled={loadingStates[wallet.address] || isRosterFull()}
                        >
                          {loadingStates[wallet.address] ? (
                            <Loader2 className="h-4 w-4 animate-spin" />
                          ) : (
                            <>
                              <ChevronUp className="mr-1 h-4 w-4" />
                              Promote
                            </>
                          )}
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          onClick={() => handleRemove(wallet.address)}
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </div>
                    </div>
                  </CardContent>
                </Card>
              ))}
            </div>
          )}
        </TabsContent>

        {/* Discover Tab */}
        <TabsContent value="discover" className="space-y-4">
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
                  <Switch checked={minWinRate} onCheckedChange={setMinWinRate} />
                  <span className="text-sm">Min win rate 55%</span>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => refetchDiscover()}
                  disabled={isLoadingDiscover}
                  className="ml-auto"
                >
                  <RefreshCw className={`mr-2 h-4 w-4 ${isLoadingDiscover ? 'animate-spin' : ''}`} />
                  Refresh
                </Button>
              </div>
            </CardContent>
          </Card>

          {/* Error State */}
          {discoverError && (
            <Card className="border-destructive">
              <CardContent className="p-6 text-center">
                <AlertCircle className="h-8 w-8 mx-auto mb-2 text-destructive" />
                <p className="text-destructive font-medium">Failed to load wallets</p>
                <p className="text-sm text-muted-foreground mt-1">
                  {discoverError instanceof Error ? discoverError.message : 'Please try again'}
                </p>
                <Button variant="outline" size="sm" className="mt-4" onClick={() => refetchDiscover()}>
                  Retry
                </Button>
              </CardContent>
            </Card>
          )}

          {/* Loading State */}
          {isLoadingDiscover && !discoverError && (
            <div className="space-y-4">
              <div className="flex items-center gap-2">
                <Star className="h-5 w-5 text-yellow-500" />
                <h2 className="text-xl font-semibold">Top Performers</h2>
              </div>
              {[1, 2, 3].map((i) => (
                <Card key={i}>
                  <CardContent className="p-6">
                    <div className="flex items-center gap-4">
                      <Skeleton className="h-10 w-10 rounded-full" />
                      <div className="flex-1 space-y-2">
                        <Skeleton className="h-4 w-32" />
                        <Skeleton className="h-3 w-24" />
                      </div>
                      <Skeleton className="h-8 w-24" />
                    </div>
                  </CardContent>
                </Card>
              ))}
            </div>
          )}

          {/* Discoverable Wallets */}
          {!isLoadingDiscover && !discoverError && (
            <div className="space-y-4">
              <div className="flex items-center gap-2">
                <Star className="h-5 w-5 text-yellow-500" />
                <h2 className="text-xl font-semibold">Top Performers</h2>
                <span className="text-sm text-muted-foreground">
                  ({filteredDiscoverable.length} wallets)
                </span>
              </div>

              <div className="grid gap-4">
                {filteredDiscoverable.map((wallet, index) => {
                  const equityCurve = equityCurves[wallet.address];

                  return (
                    <Card key={wallet.address} className="hover:border-primary transition-colors">
                      <CardContent className="p-6">
                        <div className="flex flex-col gap-6">
                          <div className="flex flex-col lg:flex-row lg:items-center gap-6">
                            {/* Rank & Address */}
                            <div className="flex items-center gap-4">
                              <div className="flex h-10 w-10 items-center justify-center rounded-full bg-primary text-primary-foreground font-bold">
                                #{wallet.rank || index + 1}
                              </div>
                              <div>
                                <p className="font-medium font-mono">{shortenAddress(wallet.address)}</p>
                                <span className={`text-xs px-2 py-0.5 rounded-full ${predictionColors[wallet.prediction]}`}>
                                  {predictionLabels[wallet.prediction]} ({wallet.confidence}%)
                                </span>
                              </div>
                            </div>

                            {/* Metrics */}
                            <div className="grid grid-cols-4 gap-4 flex-1 text-sm">
                              <div>
                                <p className="text-xs text-muted-foreground">ROI (30d)</p>
                                <p className={`font-medium ${wallet.roi_30d >= 0 ? 'text-profit' : 'text-loss'}`}>
                                  {wallet.roi_30d >= 0 ? '+' : ''}{Number(wallet.roi_30d).toFixed(1)}%
                                </p>
                              </div>
                              <div>
                                <p className="text-xs text-muted-foreground">Sharpe</p>
                                <p className="font-medium">{Number(wallet.sharpe_ratio).toFixed(2)}</p>
                              </div>
                              <div>
                                <p className="text-xs text-muted-foreground">Win Rate</p>
                                <p className="font-medium">{Number(wallet.win_rate).toFixed(1)}%</p>
                              </div>
                              <div>
                                <p className="text-xs text-muted-foreground">Max DD</p>
                                <p className="font-medium text-loss">{Number(wallet.max_drawdown).toFixed(1)}%</p>
                              </div>
                            </div>

                            {/* Actions */}
                            <div className="flex gap-2">
                              <Button
                                variant="secondary"
                                size="sm"
                                onClick={() => handleAddToBench(wallet)}
                                disabled={loadingStates[wallet.address]}
                              >
                                {loadingStates[wallet.address] ? (
                                  <Loader2 className="h-4 w-4 animate-spin" />
                                ) : (
                                  <>
                                    <Plus className="mr-1 h-4 w-4" />
                                    Add to Bench
                                  </>
                                )}
                              </Button>
                              <Button
                                variant="default"
                                size="sm"
                                onClick={() => handleCopyClick(wallet)}
                              >
                                Copy Now
                              </Button>
                            </div>
                          </div>

                          {/* Equity Curve */}
                          {equityCurve && (
                            <div className="border rounded-lg p-2 bg-muted/20">
                              <EquityCurve data={equityCurve} height={80} />
                            </div>
                          )}
                        </div>
                      </CardContent>
                    </Card>
                  );
                })}

                {filteredDiscoverable.length === 0 && (
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
          )}
        </TabsContent>
      </Tabs>

      {/* Copy Modal */}
      <CopyWalletModal
        wallet={selectedWallet ? {
          address: selectedWallet.address,
          roi30d: selectedWallet.roi_30d,
          sharpe: selectedWallet.sharpe_ratio,
          winRate: selectedWallet.win_rate,
          trades: selectedWallet.total_trades,
          confidence: selectedWallet.confidence,
        } : null}
        isOpen={copyModalOpen}
        onClose={() => {
          setCopyModalOpen(false);
          setSelectedWallet(null);
        }}
        onConfirm={handleCopyConfirm}
        rosterCount={activeWallets.length}
      />
    </div>
  );
}
