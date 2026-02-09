'use client';

import { useEffect, useMemo } from 'react';
import { useSearchParams, useRouter } from 'next/navigation';
import Link from 'next/link';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Skeleton } from '@/components/ui/skeleton';
import { PortfolioSummary, WalletCard, ManualPositions, AutomationPanel } from '@/components/trading';
import { AllocationAdjustmentPanel } from '@/components/allocations/AllocationAdjustmentPanel';
import { useDemoPortfolioStore, DemoPosition } from '@/stores/demo-portfolio-store';
import { useModeStore } from '@/stores/mode-store';
import { useToastStore } from '@/stores/toast-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import {
  useAllocationsQuery,
  usePromoteAllocationMutation,
  useDemoteAllocationMutation,
  useRemoveAllocationMutation,
  usePinAllocationMutation,
  useUnpinAllocationMutation,
} from '@/hooks/queries/useAllocationsQuery';
import { shortenAddress, formatCurrency, cn, ratioOrPercentToPercent } from '@/lib/utils';
import {
  TrendingUp,
  TrendingDown,
  Eye,
  Star,
  Search,
  Plus,
  RefreshCw,
  TestTube2,
  History,
  Bot,
} from 'lucide-react';
import { useDiscoverWalletsQuery } from '@/hooks/queries/useDiscoverQuery';
import type { WorkspaceAllocation, DiscoveredWallet } from '@/types/api';

// Position format expected by WalletCard
interface WalletPosition {
  id: string;
  marketId: string;
  marketQuestion?: string;
  outcome: 'yes' | 'no';
  quantity: number;
  entryPrice: number;
  currentPrice: number;
  pnl: number;
  pnlPercent: number;
}

interface TradingWallet {
  address: string;
  label?: string;
  tier: 'active' | 'bench';
  copySettings: {
    copy_behavior: 'copy_all' | 'events_only' | 'arb_threshold';
    allocation_pct: number;
    max_position_size: number;
    arb_threshold_pct?: number;
  };
  roi30d: number;
  sharpe: number;
  winRate: number;
  trades: number;
  maxDrawdown: number;
  confidence: number;
  addedAt: string;
  pinned?: boolean;
  pinnedAt?: string;
  probationUntil?: string;
  isAutoSelected?: boolean;
  consecutiveLosses?: number;
}

// Convert demo position to wallet position format
function toWalletPosition(p: DemoPosition): WalletPosition {
  const pnl = (p.currentPrice - p.entryPrice) * p.quantity;
  const pnlPercent = ((p.currentPrice - p.entryPrice) / p.entryPrice) * 100;
  return {
    id: p.id,
    marketId: p.marketId,
    marketQuestion: p.marketQuestion,
    outcome: p.outcome,
    quantity: p.quantity,
    entryPrice: p.entryPrice,
    currentPrice: p.currentPrice,
    pnl,
    pnlPercent,
  };
}

function toTradingWallet(allocation: WorkspaceAllocation, discovered?: DiscoveredWallet): TradingWallet {
  const hasBacktest = allocation.backtest_roi != null && allocation.backtest_roi !== 0;
  return {
    address: allocation.wallet_address,
    label: allocation.wallet_label,
    tier: allocation.tier,
    copySettings: {
      copy_behavior: allocation.copy_behavior,
      allocation_pct: allocation.allocation_pct,
      max_position_size: allocation.max_position_size ?? 100,
      arb_threshold_pct: allocation.arb_threshold_pct,
    },
    roi30d: hasBacktest
      ? ratioOrPercentToPercent(allocation.backtest_roi)
      : (discovered ? Number(discovered.roi_30d) : 0),
    sharpe: allocation.backtest_sharpe ?? (discovered ? Number(discovered.sharpe_ratio) : 0),
    winRate: hasBacktest
      ? ratioOrPercentToPercent(allocation.backtest_win_rate)
      : (discovered ? Number(discovered.win_rate) : 0),
    trades: discovered?.total_trades ?? 0,
    maxDrawdown: discovered ? Number(discovered.max_drawdown) : 0,
    confidence: allocation.confidence_score ?? (discovered?.confidence ?? 0),
    addedAt: allocation.added_at,
    pinned: allocation.pinned,
    pinnedAt: allocation.pinned_at,
    probationUntil: allocation.probation_until,
    isAutoSelected: allocation.auto_assigned,
    consecutiveLosses: allocation.consecutive_losses,
  };
}

export default function TradingPage() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const toast = useToastStore();
  const { currentWorkspace } = useWorkspaceStore();
  const { mode } = useModeStore();
  const isDemo = mode === 'demo';

  // Get tab from URL, default to 'active'
  const currentTab = searchParams.get('tab') || 'active';

  const { data: allocations = [] } = useAllocationsQuery(currentWorkspace?.id, mode);
  const { data: discoveredWallets = [] } = useDiscoverWalletsQuery(mode, {
    minTrades: 1,
    limit: 250,
  });
  const promoteMutation = usePromoteAllocationMutation(currentWorkspace?.id, mode);
  const demoteMutation = useDemoteAllocationMutation(currentWorkspace?.id, mode);
  const removeMutation = useRemoveAllocationMutation(currentWorkspace?.id, mode);
  const pinMutation = usePinAllocationMutation(currentWorkspace?.id, mode);
  const unpinMutation = useUnpinAllocationMutation(currentWorkspace?.id, mode);

  // Build lookup map of discovered wallets by address
  const discoveryMap = useMemo(() => {
    const map = new Map<string, DiscoveredWallet>();
    for (const dw of discoveredWallets) {
      map.set(dw.address.toLowerCase(), dw);
    }
    return map;
  }, [discoveredWallets]);

  const activeWallets = allocations.filter((a) => a.tier === 'active').map((a) =>
    toTradingWallet(a, discoveryMap.get(a.wallet_address.toLowerCase()))
  );
  const benchWallets = allocations.filter((a) => a.tier === 'bench').map((a) =>
    toTradingWallet(a, discoveryMap.get(a.wallet_address.toLowerCase()))
  );
  const isRosterFull = () => activeWallets.length >= 5;

  // Demo portfolio store
  const {
    positions: demoPositions,
    closedPositions: demoClosedPositions,
    isLoading: isLoadingPositions,
    fetchAll,
    closePosition: closeDemoPosition,
    getTotalValue,
    getTotalPnl,
  } = useDemoPortfolioStore();

  // Fetch positions on mount
  useEffect(() => {
    if (isDemo) {
      fetchAll();
    }
  }, [isDemo, fetchAll]);

  // Group positions by wallet address
  const positionsByWallet = useMemo(() => {
    const grouped: Record<string, WalletPosition[]> = {};
    demoPositions.forEach((p) => {
      const wallet = p.walletAddress || 'manual';
      if (!grouped[wallet]) {
        grouped[wallet] = [];
      }
      grouped[wallet].push(toWalletPosition(p));
    });
    return grouped;
  }, [demoPositions]);

  // Manual positions (no source wallet)
  const manualPositions = positionsByWallet['manual'] || [];

  // Get balance from store
  const { balance: demoBalance } = useDemoPortfolioStore();

  // Calculate summary stats
  const totalValue = isDemo ? getTotalValue() : 0;
  const totalPnl = isDemo ? getTotalPnl() : 0;
  const positionCount = demoPositions.length;
  const closedCount = demoClosedPositions.length;
  const winCount = demoClosedPositions.filter((p) => (p.realizedPnl || 0) > 0).length;
  const winRate = closedCount > 0 ? (winCount / closedCount) * 100 : 0;
  const realizedPnl = demoClosedPositions.reduce((sum, p) => sum + (p.realizedPnl || 0), 0);

  // Handle tab change
  const handleTabChange = (value: string) => {
    router.push(`/trading?tab=${value}`, { scroll: false });
  };

  // Handle wallet actions
  const handlePromote = (address: string) => {
    if (isRosterFull()) {
      toast.error('Roster Full', 'Demote a wallet from Active first');
      return;
    }
    promoteMutation.mutate(address, {
      onSuccess: () => toast.success('Promoted!', `${shortenAddress(address)} is now active`),
      onError: () => toast.error('Promotion Failed', 'Could not promote wallet'),
    });
  };

  const handleDemote = (address: string) => {
    demoteMutation.mutate(address, {
      onSuccess: () => toast.info('Demoted', `${shortenAddress(address)} moved to Watching`),
      onError: () => toast.error('Demotion Failed', 'Could not demote wallet'),
    });
  };

  const handleRemove = (address: string) => {
    removeMutation.mutate(address, {
      onSuccess: () => toast.info('Removed', `${shortenAddress(address)} removed from Watching`),
      onError: () => toast.error('Remove Failed', 'Could not remove wallet'),
    });
  };

  const handleClosePosition = (id: string) => {
    const position = demoPositions.find((p) => p.id === id);
    if (position) {
      closeDemoPosition(id, position.currentPrice);
      toast.success('Position Closed', 'Position has been closed');
    }
  };

  // Pin/Unpin handlers
  const handlePin = (address: string) => {
    pinMutation.mutate(address, {
      onSuccess: () => toast.success('Wallet Pinned', `${shortenAddress(address)} is protected from auto-demotion`),
      onError: () => toast.error('Pin Failed', 'Could not pin wallet'),
    });
  };

  const handleUnpin = (address: string) => {
    unpinMutation.mutate(address, {
      onSuccess: () => toast.info('Wallet Unpinned', `${shortenAddress(address)} can now be auto-demoted`),
      onError: () => toast.error('Unpin Failed', 'Could not unpin wallet'),
    });
  };

  // Count pinned wallets
  const pinnedCount = activeWallets.filter((w) => w.pinned).length;
  const maxPins = 3;
  const pinsRemaining = maxPins - pinnedCount;

  return (
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-4">
          <div>
            <h1 className="text-3xl font-bold tracking-tight flex items-center gap-2">
              <TrendingUp className="h-8 w-8" />
              Trading
            </h1>
            <p className="text-muted-foreground">
              Manage your copy trading wallets and positions
            </p>
          </div>
          {isDemo && (
            <div className="flex items-center gap-2 px-3 py-1.5 rounded-full bg-demo/10 text-demo text-sm font-medium">
              <TestTube2 className="h-4 w-4" />
              Demo Mode
            </div>
          )}
        </div>
        <div className="flex gap-2">
          {isDemo && (
            <Button variant="outline" size="sm" onClick={() => fetchAll()}>
              <RefreshCw className="mr-2 h-4 w-4" />
              Refresh
            </Button>
          )}
          <Link href="/discover">
            <Button>
              <Search className="mr-2 h-4 w-4" />
              Discover Wallets
            </Button>
          </Link>
        </div>
      </div>

      {/* Portfolio Summary */}
      <PortfolioSummary
        totalValue={totalValue}
        totalPnl={totalPnl}
        positionCount={positionCount}
        winRate={winRate}
        realizedPnl={realizedPnl}
        availableBalance={isDemo ? demoBalance : undefined}
        isDemo={isDemo}
      />

      {/* Tabs */}
      <Tabs value={currentTab} onValueChange={handleTabChange}>
        <TabsList>
          <TabsTrigger value="active" className="flex items-center gap-2">
            <Star className="h-4 w-4" />
            Active ({activeWallets.length}/5)
          </TabsTrigger>
          <TabsTrigger value="watching" className="flex items-center gap-2">
            <Eye className="h-4 w-4" />
            Watching ({benchWallets.length})
          </TabsTrigger>
          <TabsTrigger value="closed" className="flex items-center gap-2">
            <History className="h-4 w-4" />
            Closed ({closedCount})
          </TabsTrigger>
          <TabsTrigger value="automation" className="flex items-center gap-2">
            <Bot className="h-4 w-4" />
            Automation
          </TabsTrigger>
        </TabsList>

        {/* Active Tab */}
        <TabsContent value="active" className="space-y-4">
          {isLoadingPositions ? (
            <div className="space-y-4">
              {[1, 2].map((i) => (
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
          ) : activeWallets.length === 0 ? (
            <Card>
              <CardContent className="p-12 text-center">
                <Star className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
                <h3 className="text-lg font-medium mb-2">No active wallets</h3>
                <p className="text-muted-foreground mb-4">
                  Promote wallets from Watching or discover new wallets to start copying
                </p>
                <div className="flex gap-2 justify-center">
                  <Button variant="outline" onClick={() => handleTabChange('watching')}>
                    <Eye className="mr-2 h-4 w-4" />
                    View Watching
                  </Button>
                  <Link href="/discover">
                    <Button>
                      <Search className="mr-2 h-4 w-4" />
                      Discover Wallets
                    </Button>
                  </Link>
                </div>
              </CardContent>
            </Card>
          ) : (
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
              {activeWallets.map((wallet) => (
                <WalletCard
                  key={wallet.address}
                  wallet={wallet}
                  positions={positionsByWallet[wallet.address] || []}
                  onDemote={handleDemote}
                  onClosePosition={handleClosePosition}
                  onPin={handlePin}
                  onUnpin={handleUnpin}
                  isActive={true}
                  isRosterFull={isRosterFull()}
                  pinsRemaining={pinsRemaining}
                  maxPins={maxPins}
                />
              ))}

              {/* Empty slots */}
              {Array.from({ length: 5 - activeWallets.length }).map((_, i) => (
                <Card key={`empty-${i}`} className="border-dashed">
                  <CardContent className="p-6 flex flex-col items-center justify-center min-h-[200px] text-center">
                    <div className="h-12 w-12 rounded-full bg-muted flex items-center justify-center mb-4">
                      <Plus className="h-6 w-6 text-muted-foreground" />
                    </div>
                    <p className="font-medium text-muted-foreground mb-2">
                      Slot {activeWallets.length + i + 1} Available
                    </p>
                    <p className="text-sm text-muted-foreground mb-4">
                      Add a wallet from Watching to start copying
                    </p>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => handleTabChange('watching')}
                    >
                      Browse Watching
                    </Button>
                  </CardContent>
                </Card>
              ))}
            </div>
          )}

          {/* Manual Positions Section */}
          {manualPositions.length > 0 && (
            <ManualPositions
              positions={manualPositions}
              onClosePosition={handleClosePosition}
            />
          )}

          {/* Risk-Based Allocation Adjustment */}
          {activeWallets.length > 0 && (
            <AllocationAdjustmentPanel tier="active" className="mt-6" />
          )}
        </TabsContent>

        {/* Watching Tab */}
        <TabsContent value="watching" className="space-y-4">
          {benchWallets.length === 0 ? (
            <Card>
              <CardContent className="p-12 text-center">
                <Eye className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
                <h3 className="text-lg font-medium mb-2">No wallets being watched</h3>
                <p className="text-muted-foreground mb-4">
                  Discover promising wallets to monitor before copying
                </p>
                <Link href="/discover">
                  <Button>
                    <Search className="mr-2 h-4 w-4" />
                    Discover Wallets
                  </Button>
                </Link>
              </CardContent>
            </Card>
          ) : (
            <div className="grid gap-4">
              {benchWallets.map((wallet) => (
                <WalletCard
                  key={wallet.address}
                  wallet={wallet}
                  positions={[]}
                  onPromote={handlePromote}
                  onRemove={() => handleRemove(wallet.address)}
                  isActive={false}
                  isRosterFull={isRosterFull()}
                />
              ))}
            </div>
          )}

          {/* Risk-Based Allocation Adjustment */}
          {benchWallets.length > 0 && (
            <AllocationAdjustmentPanel tier="bench" className="mt-6" />
          )}
        </TabsContent>

        {/* Closed Positions Tab */}
        <TabsContent value="closed" className="space-y-4">
          {demoClosedPositions.length === 0 ? (
            <Card>
              <CardContent className="p-12 text-center">
                <History className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
                <h3 className="text-lg font-medium mb-2">No closed positions</h3>
                <p className="text-muted-foreground">
                  Your realized gains and losses will appear here after closing positions.
                </p>
              </CardContent>
            </Card>
          ) : (
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center justify-between">
                  <span>Closed Positions</span>
                  <span className={cn(
                    'text-lg font-bold',
                    realizedPnl >= 0 ? 'text-profit' : 'text-loss'
                  )}>
                    Total: {formatCurrency(realizedPnl, { showSign: true })}
                  </span>
                </CardTitle>
              </CardHeader>
              <CardContent>
                <div className="overflow-x-auto">
                  <table className="w-full">
                    <thead className="border-b bg-muted/50">
                      <tr>
                        <th className="text-left p-4 font-medium">Market</th>
                        <th className="text-left p-4 font-medium">Outcome</th>
                        <th className="text-right p-4 font-medium">Entry</th>
                        <th className="text-right p-4 font-medium">Exit</th>
                        <th className="text-right p-4 font-medium">Size</th>
                        <th className="text-right p-4 font-medium">Realized P&L</th>
                        <th className="text-right p-4 font-medium">Source</th>
                        <th className="text-right p-4 font-medium">Closed</th>
                      </tr>
                    </thead>
                    <tbody>
                      {demoClosedPositions.map((position) => {
                        const pnl = position.realizedPnl || 0;
                        const sourceWallet = position.walletAddress
                          ? [...activeWallets, ...benchWallets].find(
                              (w) => w.address === position.walletAddress
                            )
                          : null;

                        return (
                          <tr key={position.id} className="border-b hover:bg-muted/30">
                            <td className="p-4">
                              <p className="font-medium text-sm">
                                {position.marketQuestion || position.marketId}
                              </p>
                            </td>
                            <td className="p-4">
                              <span
                                className={cn(
                                  'px-2 py-1 rounded text-xs font-medium uppercase',
                                  position.outcome === 'yes'
                                    ? 'bg-profit/10 text-profit'
                                    : 'bg-loss/10 text-loss'
                                )}
                              >
                                {position.outcome}
                              </span>
                            </td>
                            <td className="p-4 text-right tabular-nums">
                              ${position.entryPrice.toFixed(2)}
                            </td>
                            <td className="p-4 text-right tabular-nums">
                              ${position.exitPrice?.toFixed(2) || position.currentPrice?.toFixed(2) || '-'}
                            </td>
                            <td className="p-4 text-right tabular-nums">
                              {formatCurrency(position.quantity * position.entryPrice)}
                            </td>
                            <td className="p-4 text-right">
                              <span
                                className={cn(
                                  'tabular-nums font-medium',
                                  pnl >= 0 ? 'text-profit' : 'text-loss'
                                )}
                              >
                                {formatCurrency(pnl, { showSign: true })}
                              </span>
                            </td>
                            <td className="p-4 text-right text-muted-foreground text-sm">
                              {position.walletLabel || (position.walletAddress ? shortenAddress(position.walletAddress) : 'Manual')}
                            </td>
                            <td className="p-4 text-right text-muted-foreground text-sm">
                              {position.closedAt
                                ? new Date(position.closedAt).toLocaleDateString()
                                : '-'}
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
              </CardContent>
            </Card>
          )}
        </TabsContent>

        {/* Automation Tab */}
        <TabsContent value="automation" className="space-y-4">
          <AutomationPanel
            workspaceId={currentWorkspace?.id ?? ''}
            onRefresh={() => fetchAll()}
          />
        </TabsContent>
      </Tabs>
    </div>
  );
}
