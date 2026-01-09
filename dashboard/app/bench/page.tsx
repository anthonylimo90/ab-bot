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
import { EquityCurve } from '@/components/charts/EquityCurve';
import { CopyWalletModal } from '@/components/modals/CopyWalletModal';
import { useRosterStore, createRosterWallet } from '@/stores/roster-store';
import { useToastStore } from '@/stores/toast-store';
import { shortenAddress } from '@/lib/utils';
import {
  UserCheck,
  Search,
  Plus,
  Check,
  ChevronUp,
  Loader2,
  Star,
  Trash2,
  ArrowRight,
} from 'lucide-react';
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

// Mock discovery data
const discoverableWallets = [
  {
    address: '0x5678901234abcdef5678901234abcdef56789012',
    roi30d: 29.4,
    sharpe: 1.5,
    trades: 234,
    winRate: 64,
    maxDrawdown: -15.3,
    confidence: 65,
    prediction: 'MODERATE' as const,
  },
  {
    address: '0x9876543210fedcba9876543210fedcba98765432',
    roi30d: 22.8,
    sharpe: 1.3,
    trades: 78,
    winRate: 61,
    maxDrawdown: -18.5,
    confidence: 52,
    prediction: 'LOW_POTENTIAL' as const,
  },
  {
    address: '0xfedcba9876543210fedcba9876543210fedcba98',
    roi30d: 52.1,
    sharpe: 2.6,
    trades: 45,
    winRate: 73,
    maxDrawdown: -9.2,
    confidence: 88,
    prediction: 'HIGH_POTENTIAL' as const,
  },
];

const predictionColors = {
  HIGH_POTENTIAL: 'text-profit bg-profit/10',
  MODERATE: 'text-yellow-500 bg-yellow-500/10',
  LOW_POTENTIAL: 'text-loss bg-loss/10',
};

const predictionLabels = {
  HIGH_POTENTIAL: 'High Potential',
  MODERATE: 'Moderate',
  LOW_POTENTIAL: 'Low Potential',
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
  const [selectedWallet, setSelectedWallet] = useState<typeof discoverableWallets[0] | null>(null);

  // Equity curves
  const equityCurves = useMemo(() => {
    const all = [...benchWallets, ...discoverableWallets];
    return all.reduce((acc, w) => {
      acc[w.address] = generateEquityCurve(30, w.roi30d);
      return acc;
    }, {} as Record<string, { time: string; value: number }[]>);
  }, [benchWallets]);

  // Filter discoverable wallets (exclude already tracked)
  const filteredDiscoverable = useMemo(() => {
    const trackedAddresses = new Set([
      ...activeWallets.map((w) => w.address),
      ...benchWallets.map((w) => w.address),
    ]);

    let result = discoverableWallets.filter(
      (w) => !trackedAddresses.has(w.address)
    );

    if (minWinRate) {
      result = result.filter((w) => w.winRate >= 55);
    }

    result.sort((a, b) => {
      switch (sortBy) {
        case 'roi': return b.roi30d - a.roi30d;
        case 'sharpe': return b.sharpe - a.sharpe;
        case 'winRate': return b.winRate - a.winRate;
        case 'trades': return b.trades - a.trades;
        default: return 0;
      }
    });

    return result;
  }, [sortBy, minWinRate, activeWallets, benchWallets]);

  const handleAddToBench = async (wallet: typeof discoverableWallets[0]) => {
    setLoadingStates((prev) => ({ ...prev, [wallet.address]: true }));
    await new Promise((r) => setTimeout(r, 500));

    const rosterWallet = createRosterWallet(wallet.address, {
      roi30d: wallet.roi30d,
      sharpe: wallet.sharpe,
      winRate: wallet.winRate,
      trades: wallet.trades,
      maxDrawdown: wallet.maxDrawdown,
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

  const handleCopyClick = (wallet: typeof discoverableWallets[0]) => {
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
        roi30d: wallet.roi30d,
        sharpe: wallet.sharpe,
        winRate: wallet.winRate,
        trades: wallet.trades,
        maxDrawdown: wallet.maxDrawdown,
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
                <div className="ml-auto flex items-center gap-2">
                  <Switch checked={minWinRate} onCheckedChange={setMinWinRate} />
                  <span className="text-sm">Min win rate 55%</span>
                </div>
              </div>
            </CardContent>
          </Card>

          {/* Discoverable Wallets */}
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
                              #{index + 1}
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
                              <p className="text-xs text-muted-foreground">Max DD</p>
                              <p className="font-medium text-loss">{wallet.maxDrawdown}%</p>
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
        </TabsContent>
      </Tabs>

      {/* Copy Modal */}
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
        rosterCount={activeWallets.length}
      />
    </div>
  );
}
