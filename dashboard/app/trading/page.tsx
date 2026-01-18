'use client';

import { useEffect, useMemo } from 'react';
import { useSearchParams, useRouter } from 'next/navigation';
import Link from 'next/link';
import { Card, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Skeleton } from '@/components/ui/skeleton';
import { PortfolioSummary, WalletCard, ManualPositions } from '@/components/trading';
import { useRosterStore, RosterWallet } from '@/stores/roster-store';
import { useDemoPortfolioStore, DemoPosition } from '@/stores/demo-portfolio-store';
import { useModeStore } from '@/stores/mode-store';
import { useToastStore } from '@/stores/toast-store';
import { shortenAddress } from '@/lib/utils';
import {
  TrendingUp,
  Eye,
  Star,
  Search,
  Plus,
  RefreshCw,
  TestTube2,
} from 'lucide-react';

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

export default function TradingPage() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const toast = useToastStore();
  const { mode } = useModeStore();
  const isDemo = mode === 'demo';

  // Get tab from URL, default to 'active'
  const currentTab = searchParams.get('tab') || 'active';

  // Roster store
  const {
    activeWallets,
    benchWallets,
    promoteToActive,
    demoteToBench,
    removeFromBench,
    isRosterFull,
  } = useRosterStore();

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

  // Calculate summary stats
  const totalValue = isDemo ? getTotalValue() : 0;
  const totalPnl = isDemo ? getTotalPnl() : 0;
  const positionCount = demoPositions.length;
  const closedCount = demoClosedPositions.length;
  const winCount = demoClosedPositions.filter((p) => (p.realizedPnl || 0) > 0).length;
  const winRate = closedCount > 0 ? (winCount / closedCount) * 100 : 0;

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
    promoteToActive(address);
    toast.success('Promoted!', `${shortenAddress(address)} is now active`);
  };

  const handleDemote = (address: string) => {
    demoteToBench(address);
    toast.info('Demoted', `${shortenAddress(address)} moved to Watching`);
  };

  const handleRemove = (address: string) => {
    removeFromBench(address);
    toast.info('Removed', `${shortenAddress(address)} removed from Watching`);
  };

  const handleClosePosition = (id: string) => {
    const position = demoPositions.find((p) => p.id === id);
    if (position) {
      closeDemoPosition(id, position.currentPrice);
      toast.success('Position Closed', 'Position has been closed');
    }
  };

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
                  isActive={true}
                  isRosterFull={isRosterFull()}
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
        </TabsContent>
      </Tabs>
    </div>
  );
}
