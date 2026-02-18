"use client";

import { useMemo, useState, useCallback } from "react";
import { useSearchParams, useRouter } from "next/navigation";
import Link from "next/link";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Skeleton } from "@/components/ui/skeleton";
import {
  PortfolioSummary,
  WalletCard,
  ManualPositions,
  AutomationPanel,
} from "@/components/trading";
import { AllocationAdjustmentPanel } from "@/components/allocations/AllocationAdjustmentPanel";
import { ErrorBoundary } from "@/components/shared/ErrorBoundary";
import { useToastStore } from "@/stores/toast-store";
import { useWorkspaceStore } from "@/stores/workspace-store";
import {
  usePositionsQuery,
  useClosePositionMutation,
} from "@/hooks/queries/usePositionsQuery";
import { useWalletStore } from "@/stores/wallet-store";
import { useWalletBalanceQuery } from "@/hooks/queries/useWalletsQuery";
import type { Position, WalletPosition, PositionState } from "@/types/api";
import {
  useAllocationsQuery,
  usePromoteAllocationMutation,
  useDemoteAllocationMutation,
  useRemoveAllocationMutation,
  usePinAllocationMutation,
  useUnpinAllocationMutation,
} from "@/hooks/queries/useAllocationsQuery";
import {
  shortenAddress,
  formatCurrency,
  cn,
  ratioOrPercentToPercent,
} from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  TrendingUp,
  Eye,
  Star,
  Search,
  Plus,
  History,
  Bot,
} from "lucide-react";
import { useDiscoverWalletsQuery } from "@/hooks/queries/useDiscoverQuery";
import type { WorkspaceAllocation, DiscoveredWallet } from "@/types/api";

interface TradingWallet {
  address: string;
  label?: string;
  tier: "active" | "bench";
  copySettings: {
    copy_behavior: "copy_all" | "events_only" | "arb_threshold";
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
  strategyType?: string;
  stalenessDays?: number;
  compositeScore?: number;
}

/** Position lifecycle state badge configuration */
const POSITION_STATE_CONFIG: Record<
  PositionState,
  { label: string; variant: "default" | "secondary" | "destructive" | "outline" }
> = {
  pending: { label: "Pending", variant: "outline" },
  open: { label: "Open", variant: "default" },
  exit_ready: { label: "Exit Ready", variant: "secondary" },
  closing: { label: "Closing", variant: "secondary" },
  closed: { label: "Closed", variant: "outline" },
  entry_failed: { label: "Entry Failed", variant: "destructive" },
  exit_failed: { label: "Exit Failed", variant: "destructive" },
  stalled: { label: "Stalled", variant: "destructive" },
};

function PositionStateBadge({ state }: { state?: PositionState }) {
  if (!state) return null;
  const config = POSITION_STATE_CONFIG[state];
  return (
    <Badge variant={config.variant} className="text-xs">
      {config.label}
    </Badge>
  );
}

interface ClosedPositionDisplay {
  id: string;
  marketQuestion?: string;
  marketId: string;
  outcome: string;
  entryPrice: number;
  exitPrice?: number;
  quantity: number;
  realizedPnl: number;
  walletAddress?: string;
  walletLabel?: string;
  closedAt?: string;
  state?: PositionState;
  entryFees?: number;
  exitFees?: number;
}

function livePositionToWalletPosition(p: Position): WalletPosition {
  return {
    id: p.id,
    marketId: p.market_id,
    marketQuestion: undefined,
    outcome: p.outcome as "yes" | "no",
    quantity: p.quantity,
    entryPrice: p.entry_price,
    currentPrice: p.current_price,
    pnl: p.unrealized_pnl,
    pnlPercent: p.unrealized_pnl_pct,
  };
}

function toTradingWallet(
  allocation: WorkspaceAllocation,
  discovered?: DiscoveredWallet,
): TradingWallet {
  const hasBacktest =
    allocation.backtest_roi != null && allocation.backtest_roi !== 0;
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
      : discovered
        ? Number(discovered.roi_30d)
        : 0,
    sharpe:
      allocation.backtest_sharpe ??
      (discovered ? Number(discovered.sharpe_ratio) : 0),
    winRate: hasBacktest
      ? ratioOrPercentToPercent(allocation.backtest_win_rate)
      : discovered
        ? Number(discovered.win_rate)
        : 0,
    trades: discovered?.total_trades ?? 0,
    maxDrawdown: discovered ? Number(discovered.max_drawdown) : 0,
    confidence: allocation.confidence_score ?? discovered?.confidence ?? 0,
    addedAt: allocation.added_at,
    pinned: allocation.pinned,
    pinnedAt: allocation.pinned_at,
    probationUntil: allocation.probation_until,
    isAutoSelected: allocation.auto_assigned,
    consecutiveLosses: allocation.consecutive_losses,
    strategyType: discovered?.strategy_type ?? undefined,
    stalenessDays: discovered?.staleness_days ?? undefined,
    compositeScore: discovered?.composite_score
      ? Number(discovered.composite_score) * 100
      : undefined,
  };
}

export default function TradingPage() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const toast = useToastStore();
  const [walletSearch, setWalletSearch] = useState("");
  const { currentWorkspace } = useWorkspaceStore();

  // Get tab from URL, default to 'active'
  const currentTab = searchParams.get("tab") || "active";

  const { data: allocations = [] } = useAllocationsQuery(currentWorkspace?.id);
  const { data: discoveredWallets = [] } = useDiscoverWalletsQuery({
    minTrades: 1,
    limit: 250,
  });
  const promoteMutation = usePromoteAllocationMutation(currentWorkspace?.id);
  const demoteMutation = useDemoteAllocationMutation(currentWorkspace?.id);
  const removeMutation = useRemoveAllocationMutation(currentWorkspace?.id);
  const pinMutation = usePinAllocationMutation(currentWorkspace?.id);
  const unpinMutation = useUnpinAllocationMutation(currentWorkspace?.id);

  // Build lookup map of discovered wallets by address
  const discoveryMap = useMemo(() => {
    const map = new Map<string, DiscoveredWallet>();
    for (const dw of discoveredWallets) {
      map.set(dw.address.toLowerCase(), dw);
    }
    return map;
  }, [discoveredWallets]);

  const allActiveWallets = useMemo(
    () =>
      allocations
        .filter((a) => a.tier === "active")
        .map((a) =>
          toTradingWallet(a, discoveryMap.get(a.wallet_address.toLowerCase())),
        ),
    [allocations, discoveryMap],
  );
  const allBenchWallets = useMemo(
    () =>
      allocations
        .filter((a) => a.tier === "bench")
        .map((a) =>
          toTradingWallet(a, discoveryMap.get(a.wallet_address.toLowerCase())),
        ),
    [allocations, discoveryMap],
  );

  // Filter wallets by search query
  const searchLower = walletSearch.toLowerCase().trim();
  const activeWallets = searchLower
    ? allActiveWallets.filter(
        (w) =>
          w.address.toLowerCase().includes(searchLower) ||
          (w.label && w.label.toLowerCase().includes(searchLower)),
      )
    : allActiveWallets;
  const benchWallets = searchLower
    ? allBenchWallets.filter(
        (w) =>
          w.address.toLowerCase().includes(searchLower) ||
          (w.label && w.label.toLowerCase().includes(searchLower)),
      )
    : allBenchWallets;
  const isRosterFull = allActiveWallets.length >= 5;

  // Positions data (single source of truth via TanStack Query)
  const { data: liveOpenPositions = [], isLoading: isLoadingPositions } =
    usePositionsQuery({ status: "open" });
  const { data: liveClosedPositions = [] } = usePositionsQuery({
    status: "closed",
  });
  const hasConnectedWallet = useWalletStore(
    (state) => state.connectedWallets.length > 0,
  );
  const { data: walletBalance } = useWalletBalanceQuery(
    hasConnectedWallet ? "active" : null,
  );

  // Close position mutation
  const closePositionMutation = useClosePositionMutation();

  const closeMutate = closePositionMutation.mutate;
  const handleClosePosition = useCallback(
    (positionId: string) => {
      closeMutate(
        { positionId },
        {
          onSuccess: () =>
            toast.success("Position Closed", "Position has been closed"),
          onError: () =>
            toast.error("Close Failed", "Could not close position"),
        },
      );
    },
    [closeMutate, toast],
  );

  // Group open positions by wallet address (normalized to lowercase)
  const positionsByWallet = useMemo(() => {
    const grouped: Record<string, WalletPosition[]> = {};
    liveOpenPositions.forEach((p) => {
      const wallet = p.source_wallet?.toLowerCase() || "manual";
      if (!grouped[wallet]) grouped[wallet] = [];
      grouped[wallet].push(livePositionToWalletPosition(p));
    });
    return grouped;
  }, [liveOpenPositions]);

  // Manual positions (no source wallet)
  const manualPositions = useMemo(
    () => positionsByWallet["manual"] || [],
    [positionsByWallet],
  );

  // Summary stats derived from position queries (no separate API call)
  const unrealizedPnl = useMemo(
    () => liveOpenPositions.reduce((sum, p) => sum + p.unrealized_pnl, 0),
    [liveOpenPositions],
  );
  const positionCount = liveOpenPositions.length;

  const { closedCount, winRate, realizedPnl } = useMemo(() => {
    const count = liveClosedPositions.length;
    const wins = liveClosedPositions.filter(
      (p) => (p.realized_pnl ?? 0) > 0,
    ).length;
    const realized = liveClosedPositions.reduce(
      (sum, p) => sum + (p.realized_pnl ?? 0),
      0,
    );
    return {
      closedCount: count,
      winRate: count > 0 ? (wins / count) * 100 : null,
      realizedPnl: realized,
    };
  }, [liveClosedPositions]);

  // Handle tab change
  const handleTabChange = useCallback(
    (value: string) => {
      router.push(`/trading?tab=${value}`, { scroll: false });
    },
    [router],
  );

  // Handle wallet actions
  const handlePromote = (address: string) => {
    if (isRosterFull) {
      toast.error("Roster Full", "Demote a wallet from Active first");
      return;
    }
    promoteMutation.mutate(address, {
      onSuccess: () =>
        toast.success("Promoted!", `${shortenAddress(address)} is now active`),
      onError: () =>
        toast.error("Promotion Failed", "Could not promote wallet"),
    });
  };

  const handleDemote = (address: string) => {
    demoteMutation.mutate(address, {
      onSuccess: () =>
        toast.info("Demoted", `${shortenAddress(address)} moved to Watching`),
      onError: () => toast.error("Demotion Failed", "Could not demote wallet"),
    });
  };

  const handleRemove = (address: string) => {
    removeMutation.mutate(address, {
      onSuccess: () =>
        toast.info(
          "Removed",
          `${shortenAddress(address)} removed from Watching`,
        ),
      onError: () => toast.error("Remove Failed", "Could not remove wallet"),
    });
  };

  // Pin/Unpin handlers
  const handlePin = (address: string) => {
    pinMutation.mutate(address, {
      onSuccess: () =>
        toast.success(
          "Wallet Pinned",
          `${shortenAddress(address)} is protected from auto-demotion`,
        ),
      onError: () => toast.error("Pin Failed", "Could not pin wallet"),
    });
  };

  const handleUnpin = (address: string) => {
    unpinMutation.mutate(address, {
      onSuccess: () =>
        toast.info(
          "Wallet Unpinned",
          `${shortenAddress(address)} can now be auto-demoted`,
        ),
      onError: () => toast.error("Unpin Failed", "Could not unpin wallet"),
    });
  };

  // Count pinned wallets
  const pinnedCount = allActiveWallets.filter((w) => w.pinned).length;
  const maxPins = 3;
  const pinsRemaining = maxPins - pinnedCount;

  const closedDisplayPositions = useMemo((): ClosedPositionDisplay[] => {
    return liveClosedPositions.map((p) => {
      // Use actual exit prices from backend when available, fallback to current_price
      const exitPrice =
        p.yes_exit_price ?? p.no_exit_price ?? p.current_price;
      return {
        id: p.id,
        marketQuestion: undefined,
        marketId: p.market_id,
        outcome: p.outcome,
        entryPrice: p.entry_price,
        exitPrice,
        quantity: p.quantity,
        realizedPnl: p.realized_pnl ?? 0,
        walletAddress: p.source_wallet,
        walletLabel: undefined,
        closedAt: p.updated_at,
        state: p.state,
        entryFees: p.entry_fees,
        exitFees: p.exit_fees,
      };
    });
  }, [liveClosedPositions]);

  return (
    <ErrorBoundary>
      <div className="space-y-6">
        {/* Page Header */}
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-3xl font-bold tracking-tight flex items-center gap-2">
              <TrendingUp className="h-8 w-8" />
              Trading
            </h1>
            <p className="text-muted-foreground">
              Manage your copy trading wallets and positions
            </p>
          </div>
          <Link href="/discover">
            <Button>
              <Search className="mr-2 h-4 w-4" />
              Discover Wallets
            </Button>
          </Link>
        </div>

        {/* Portfolio Summary */}
        <PortfolioSummary
          unrealizedPnl={unrealizedPnl}
          positionCount={positionCount}
          winRate={winRate}
          realizedPnl={realizedPnl}
          availableBalance={walletBalance?.usdc_balance ?? undefined}
          isLoading={isLoadingPositions}
        />

        {/* Search */}
        <div className="relative">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
          <Input
            type="text"
            value={walletSearch}
            onChange={(e) => setWalletSearch(e.target.value)}
            placeholder="Search wallets by address or label..."
            className="pl-10"
            aria-label="Search wallets"
          />
        </div>

        {/* Tabs */}
        <Tabs value={currentTab} onValueChange={handleTabChange}>
          <TabsList>
            <TabsTrigger value="active" className="flex items-center gap-2">
              <Star className="h-4 w-4" />
              Active ({allActiveWallets.length}/5)
            </TabsTrigger>
            <TabsTrigger value="watching" className="flex items-center gap-2">
              <Eye className="h-4 w-4" />
              Watching ({allBenchWallets.length})
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
                  <h3 className="text-lg font-medium mb-2">
                    No active wallets
                  </h3>
                  <p className="text-muted-foreground mb-4">
                    Promote wallets from Watching or discover new wallets to
                    start copying
                  </p>
                  <div className="flex gap-2 justify-center">
                    <Button
                      variant="outline"
                      onClick={() => handleTabChange("watching")}
                    >
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
                    positions={positionsByWallet[wallet.address.toLowerCase()] || []}
                    onDemote={handleDemote}
                    onPin={handlePin}
                    onUnpin={handleUnpin}
                    onClosePosition={handleClosePosition}
                    isActive={true}
                    isRosterFull={isRosterFull}
                    pinsRemaining={pinsRemaining}
                    maxPins={maxPins}
                  />
                ))}

                {/* Empty slots */}
                {!searchLower &&
                  Array.from({ length: 5 - allActiveWallets.length }).map(
                    (_, i) => (
                      <Card key={`empty-${i}`} className="border-dashed">
                        <CardContent className="p-6 flex flex-col items-center justify-center min-h-[200px] text-center">
                          <div className="h-12 w-12 rounded-full bg-muted flex items-center justify-center mb-4">
                            <Plus className="h-6 w-6 text-muted-foreground" />
                          </div>
                          <p className="font-medium text-muted-foreground mb-2">
                            Slot {allActiveWallets.length + i + 1} Available
                          </p>
                          <p className="text-sm text-muted-foreground mb-4">
                            Add a wallet from Watching to start copying
                          </p>
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={() => handleTabChange("watching")}
                          >
                            Browse Watching
                          </Button>
                        </CardContent>
                      </Card>
                    ),
                  )}
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
                  <h3 className="text-lg font-medium mb-2">
                    No wallets being watched
                  </h3>
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
              <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
                {benchWallets.map((wallet) => (
                  <WalletCard
                    key={wallet.address}
                    wallet={wallet}
                    positions={[]}
                    onPromote={handlePromote}
                    onRemove={() => handleRemove(wallet.address)}
                    isActive={false}
                    isRosterFull={isRosterFull}
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
            {closedDisplayPositions.length === 0 ? (
              <Card>
                <CardContent className="p-12 text-center">
                  <History className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
                  <h3 className="text-lg font-medium mb-2">
                    No closed positions
                  </h3>
                  <p className="text-muted-foreground">
                    Your realized gains and losses will appear here after
                    closing positions.
                  </p>
                </CardContent>
              </Card>
            ) : (
              <Card>
                <CardHeader>
                  <CardTitle className="flex items-center justify-between">
                    <span>Closed Positions</span>
                    <span
                      className={cn(
                        "text-lg font-bold",
                        realizedPnl >= 0 ? "text-profit" : "text-loss",
                      )}
                    >
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
                          <th className="text-left p-4 font-medium">State</th>
                          <th className="text-right p-4 font-medium">Entry</th>
                          <th className="text-right p-4 font-medium">Exit</th>
                          <th className="text-right p-4 font-medium">Size</th>
                          <th className="text-right p-4 font-medium">Fees</th>
                          <th className="text-right p-4 font-medium">
                            Realized P&L
                          </th>
                          <th className="text-right p-4 font-medium">Source</th>
                          <th className="text-right p-4 font-medium">Closed</th>
                        </tr>
                      </thead>
                      <tbody>
                        {closedDisplayPositions.map((position) => {
                          const totalFees =
                            (position.entryFees ?? 0) +
                            (position.exitFees ?? 0);
                          return (
                            <tr
                              key={position.id}
                              className="border-b hover:bg-muted/30"
                            >
                              <td className="p-4">
                                <p className="font-medium text-sm">
                                  {position.marketQuestion ||
                                    shortenAddress(position.marketId)}
                                </p>
                              </td>
                              <td className="p-4">
                                <span
                                  className={cn(
                                    "px-2 py-1 rounded text-xs font-medium uppercase",
                                    position.outcome === "yes"
                                      ? "bg-profit/10 text-profit"
                                      : "bg-loss/10 text-loss",
                                  )}
                                >
                                  {position.outcome}
                                </span>
                              </td>
                              <td className="p-4">
                                <PositionStateBadge state={position.state} />
                              </td>
                              <td className="p-4 text-right tabular-nums">
                                ${position.entryPrice.toFixed(2)}
                              </td>
                              <td className="p-4 text-right tabular-nums">
                                ${position.exitPrice?.toFixed(2) || "-"}
                              </td>
                              <td className="p-4 text-right tabular-nums">
                                {formatCurrency(
                                  position.quantity * position.entryPrice,
                                )}
                              </td>
                              <td className="p-4 text-right">
                                {totalFees > 0 ? (
                                  <Tooltip>
                                    <TooltipTrigger asChild>
                                      <span className="tabular-nums text-sm text-loss cursor-help">
                                        {formatCurrency(totalFees)}
                                      </span>
                                    </TooltipTrigger>
                                    <TooltipContent>
                                      <div className="text-xs space-y-1">
                                        <p>
                                          Entry:{" "}
                                          {formatCurrency(
                                            position.entryFees ?? 0,
                                          )}
                                        </p>
                                        <p>
                                          Exit:{" "}
                                          {formatCurrency(
                                            position.exitFees ?? 0,
                                          )}
                                        </p>
                                      </div>
                                    </TooltipContent>
                                  </Tooltip>
                                ) : (
                                  <span className="text-sm text-muted-foreground">
                                    -
                                  </span>
                                )}
                              </td>
                              <td className="p-4 text-right">
                                <span
                                  className={cn(
                                    "tabular-nums font-medium",
                                    position.realizedPnl >= 0
                                      ? "text-profit"
                                      : "text-loss",
                                  )}
                                >
                                  {formatCurrency(position.realizedPnl, {
                                    showSign: true,
                                  })}
                                </span>
                              </td>
                              <td className="p-4 text-right text-muted-foreground text-sm">
                                {position.walletLabel ||
                                  (position.walletAddress
                                    ? shortenAddress(position.walletAddress)
                                    : "Manual")}
                              </td>
                              <td className="p-4 text-right text-muted-foreground text-sm">
                                {position.closedAt
                                  ? new Date(
                                      position.closedAt,
                                    ).toLocaleDateString()
                                  : "-"}
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
            <AutomationPanel workspaceId={currentWorkspace?.id ?? ""} />
          </TabsContent>
        </Tabs>
      </div>
    </ErrorBoundary>
  );
}
