"use client";

import { useMemo, useState } from "react";
import { useParams, useRouter } from "next/navigation";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { useToastStore } from "@/stores/toast-store";
import {
  useWalletQuery,
  useWalletMetricsQuery,
  useWalletBalanceQuery,
  useWalletTradesQuery,
} from "@/hooks/queries/useWalletsQuery";
import { useDiscoveredWalletQuery } from "@/hooks/queries/useDiscoverQuery";
import {
  useAllocationsQuery,
  useDemoteAllocationMutation,
  usePromoteAllocationMutation,
} from "@/hooks/queries/useAllocationsQuery";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { useWalletStore } from "@/stores/wallet-store";
import {
  shortenAddress,
  formatCurrency,
  ratioOrPercentToPercent,
} from "@/lib/utils";
import {
  ArrowLeft,
  Wallet,
  TrendingUp,
  TrendingDown,
  Target,
  ChevronUp,
  ChevronDown,
  Zap,
  Activity,
  Copy,
  Check,
  Loader2,
  ExternalLink,
  Pin,
  AlertTriangle,
  Clock,
  BarChart3,
  ShoppingCart,
} from "lucide-react";
import { WalletAllocationSection } from "@/components/trading/WalletAllocationSection";
import { ErrorBoundary } from "@/components/shared/ErrorBoundary";
import { ErrorDisplay } from "@/components/shared/ErrorDisplay";
import { usePositionsQuery } from "@/hooks/queries/usePositionsQuery";
import {
  StrategyBadge,
  StalenessIndicator,
  CompositeScoreGauge,
  CalibrationChart,
  CopyPerformance,
} from "@/components/discover";

type RoiPeriod = "7d" | "30d" | "90d";

export default function WalletDetailPage() {
  const params = useParams();
  const router = useRouter();
  const address = params.address as string;
  const toast = useToastStore();
  const { currentWorkspace } = useWorkspaceStore();
  const { data: allocations = [] } = useAllocationsQuery(currentWorkspace?.id);
  const promoteMutation = usePromoteAllocationMutation(currentWorkspace?.id);
  const demoteMutation = useDemoteAllocationMutation(currentWorkspace?.id);
  const { primaryWallet } = useWalletStore();
  const { data: walletBalance } = useWalletBalanceQuery(primaryWallet);
  const balance = walletBalance?.usdc_balance ?? 0;
  const { data: livePositions = [] } = usePositionsQuery({ status: "open" });

  const [roiPeriod, setRoiPeriod] = useState<RoiPeriod>("30d");

  const storedWallet = useMemo(() => {
    return allocations.find(
      (w) => w.wallet_address.toLowerCase() === address?.toLowerCase(),
    );
  }, [allocations, address]);

  const {
    data: apiWallet,
    isLoading: isLoadingWallet,
    error: walletError,
    refetch: refetchWallet,
  } = useWalletQuery(address, !storedWallet);
  const { data: walletMetrics, isLoading: isLoadingMetrics } =
    useWalletMetricsQuery(address, !storedWallet);
  const { data: discoveredWallet, isLoading: isLoadingDiscovered } =
    useDiscoveredWalletQuery(address, !storedWallet);

  const [tradesLimit, setTradesLimit] = useState(10);
  const {
    data: walletTrades,
    isLoading: isLoadingTrades,
    error: tradesError,
    refetch: refetchTrades,
  } = useWalletTradesQuery(address, {
    limit: tradesLimit,
  });

  const [copied, setCopied] = useState(false);
  const handleCopyAddress = async () => {
    await navigator.clipboard.writeText(address);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const wallet = useMemo(() => {
    if (storedWallet) {
      const hasBacktest =
        storedWallet.backtest_roi != null && storedWallet.backtest_roi !== 0;
      return {
        address: storedWallet.wallet_address,
        label: storedWallet.wallet_label,
        tier: storedWallet.tier,
        roi30d: hasBacktest
          ? ratioOrPercentToPercent(storedWallet.backtest_roi)
          : discoveredWallet
            ? Number(discoveredWallet.roi_30d)
            : 0,
        roi7d: discoveredWallet ? Number(discoveredWallet.roi_7d) : 0,
        roi90d: discoveredWallet ? Number(discoveredWallet.roi_90d) : 0,
        sharpe:
          storedWallet.backtest_sharpe ??
          (discoveredWallet ? Number(discoveredWallet.sharpe_ratio) : 0),
        winRate: hasBacktest
          ? ratioOrPercentToPercent(storedWallet.backtest_win_rate)
          : discoveredWallet
            ? Number(discoveredWallet.win_rate)
            : 0,
        trades: discoveredWallet?.total_trades ?? 0,
        maxDrawdown: discoveredWallet
          ? Number(discoveredWallet.max_drawdown)
          : 0,
        confidence:
          storedWallet.confidence_score ?? discoveredWallet?.confidence ?? 0,
        copySettings: {
          copy_behavior: storedWallet.copy_behavior,
          allocation_pct: storedWallet.allocation_pct,
          max_position_size: storedWallet.max_position_size ?? 100,
        },
        addedAt: storedWallet.added_at,
      };
    }
    if (apiWallet) {
      return {
        address: apiWallet.address,
        label: apiWallet.label,
        tier: apiWallet.copy_enabled ? ("active" as const) : ("bench" as const),
        roi30d: ratioOrPercentToPercent(walletMetrics?.roi),
        roi7d: 0,
        roi90d: 0,
        sharpe: walletMetrics?.sharpe_ratio ?? 0,
        winRate: ratioOrPercentToPercent(apiWallet?.win_rate),
        trades: apiWallet?.total_trades ?? 0,
        maxDrawdown: ratioOrPercentToPercent(walletMetrics?.max_drawdown),
        confidence: 0,
        copySettings: {
          copy_behavior: "events_only" as const,
          allocation_pct: apiWallet.allocation_pct ?? 0,
          max_position_size: apiWallet.max_position_size ?? 100,
        },
        addedAt: apiWallet.added_at ?? new Date().toISOString(),
      };
    }
    if (discoveredWallet) {
      return {
        address: discoveredWallet.address,
        label: undefined,
        tier: "bench" as const,
        roi30d: Number(discoveredWallet.roi_30d),
        roi7d: Number(discoveredWallet.roi_7d),
        roi90d: Number(discoveredWallet.roi_90d),
        sharpe: Number(discoveredWallet.sharpe_ratio),
        winRate: Number(discoveredWallet.win_rate),
        trades: discoveredWallet.total_trades,
        maxDrawdown: Number(discoveredWallet.max_drawdown),
        confidence: discoveredWallet.confidence,
        copySettings: {
          copy_behavior: "events_only" as const,
          allocation_pct: 0,
          max_position_size: 100,
        },
        addedAt: new Date().toISOString(),
      };
    }
    return {
      address: address,
      label: undefined,
      tier: "bench" as const,
      roi30d: 0,
      roi7d: 0,
      roi90d: 0,
      sharpe: 0,
      winRate: 0,
      trades: 0,
      maxDrawdown: 0,
      confidence: 0,
      copySettings: {
        copy_behavior: "events_only" as const,
        allocation_pct: 0,
        max_position_size: 100,
      },
      addedAt: new Date().toISOString(),
    };
  }, [storedWallet, apiWallet, walletMetrics, discoveredWallet, address]);

  const walletPositionsValue = useMemo(() => {
    return livePositions
      .filter((p) => p.source_wallet?.toLowerCase() === address?.toLowerCase())
      .reduce((sum, p) => sum + p.entry_price * p.quantity, 0);
  }, [livePositions, address]);

  const isActive = allocations.some(
    (w) =>
      w.wallet_address.toLowerCase() === address?.toLowerCase() &&
      w.tier === "active",
  );
  const isBench = allocations.some(
    (w) =>
      w.wallet_address.toLowerCase() === address?.toLowerCase() &&
      w.tier === "bench",
  );
  const isLoading = isLoadingWallet || isLoadingMetrics || isLoadingDiscovered;
  const isRosterFull = useMemo(
    () => allocations.filter((a) => a.tier === "active").length >= 5,
    [allocations],
  );

  const roiByPeriod: Record<RoiPeriod, number> = {
    "7d": wallet.roi7d ?? 0,
    "30d": wallet.roi30d ?? 0,
    "90d": wallet.roi90d ?? 0,
  };
  const selectedRoi = roiByPeriod[roiPeriod];

  const tradeSummary = useMemo(() => {
    if (!walletTrades || walletTrades.length === 0) return null;
    const totalVolume = walletTrades.reduce(
      (sum, t) => sum + (t.value || 0),
      0,
    );
    const avgSize = totalVolume / walletTrades.length;
    const buys = walletTrades.filter((t) => t.side === "BUY").length;
    const sells = walletTrades.length - buys;
    return { totalVolume, avgSize, buys, sells };
  }, [walletTrades]);

  const trackingSince = useMemo(() => {
    if (!storedWallet?.added_at) return null;
    const addedDate = new Date(storedWallet.added_at);
    return Math.floor(
      (Date.now() - addedDate.getTime()) / (1000 * 60 * 60 * 24),
    );
  }, [storedWallet]);

  const handlePromote = () => {
    if (isRosterFull) {
      toast.error("Roster Full", "Demote a wallet first to make room");
      return;
    }
    promoteMutation.mutate(address, {
      onSuccess: () =>
        toast.success(
          "Promoted!",
          `${shortenAddress(address)} added to Active`,
        ),
      onError: () =>
        toast.error("Promotion Failed", "Could not promote wallet"),
    });
  };

  const handleDemote = () => {
    demoteMutation.mutate(address, {
      onSuccess: () =>
        toast.info("Demoted", `${shortenAddress(address)} moved to Bench`),
      onError: () => toast.error("Demotion Failed", "Could not demote wallet"),
    });
  };

  if (walletError && !storedWallet && !discoveredWallet) {
    return (
      <div className="space-y-6">
        <div className="flex items-center gap-4">
          <Button
            variant="ghost"
            size="icon"
            onClick={() => router.back()}
            aria-label="Go back"
          >
            <ArrowLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold tracking-tight">Wallet Details</h1>
        </div>
        <ErrorDisplay
          error={walletError}
          onRetry={() => refetchWallet()}
          variant="card"
          title="Failed to load wallet"
        />
      </div>
    );
  }

  return (
    <ErrorBoundary>
      <div className="space-y-6">
        {/* Breadcrumb & Header */}
        <div className="flex items-center gap-4">
          <Button
            variant="ghost"
            size="icon"
            onClick={() => router.back()}
            aria-label="Go back"
          >
            <ArrowLeft className="h-5 w-5" />
          </Button>
          <div className="flex-1">
            <div className="flex items-center gap-3">
              <Wallet className="h-8 w-8" />
              <div>
                {isLoading ? (
                  <>
                    <Skeleton className="h-8 w-48 mb-2" />
                    <Skeleton className="h-4 w-32" />
                  </>
                ) : (
                  <>
                    <h1 className="text-3xl font-bold tracking-tight">
                      {wallet.label || shortenAddress(address)}
                    </h1>
                    <div className="flex items-center gap-1.5">
                      <p className="text-muted-foreground font-mono">
                        {shortenAddress(address)}
                      </p>
                      <button
                        onClick={handleCopyAddress}
                        className="p-1 rounded hover:bg-muted transition-colors"
                        aria-label="Copy address"
                      >
                        {copied ? (
                          <Check className="h-3.5 w-3.5 text-profit" />
                        ) : (
                          <Copy className="h-3.5 w-3.5 text-muted-foreground" />
                        )}
                      </button>
                      <a
                        href={`https://polymarket.com/profile/${address}`}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="p-1 rounded hover:bg-muted transition-colors"
                        aria-label="View on Polymarket"
                      >
                        <ExternalLink className="h-3.5 w-3.5 text-muted-foreground" />
                      </a>
                    </div>
                    {trackingSince !== null && (
                      <p className="text-xs text-muted-foreground mt-0.5 flex items-center gap-1">
                        <Clock className="h-3 w-3" />
                        Tracking for {trackingSince} day
                        {trackingSince !== 1 ? "s" : ""}
                      </p>
                    )}
                  </>
                )}
              </div>
            </div>
          </div>
          <div className="flex items-center gap-2 flex-wrap">
            {/* Tier badge */}
            {isActive ? (
              <span className="px-3 py-1 rounded-full bg-primary text-primary-foreground text-sm font-medium">
                Active
              </span>
            ) : isBench ? (
              <span className="px-3 py-1 rounded-full bg-muted text-muted-foreground text-sm font-medium">
                Watching
              </span>
            ) : (
              <span className="px-3 py-1 rounded-full bg-muted text-muted-foreground text-sm font-medium">
                Untracked
              </span>
            )}
            {/* Status badges */}
            {storedWallet?.pinned && (
              <span className="px-2 py-1 rounded-full bg-yellow-500/10 text-yellow-600 text-xs font-medium flex items-center gap-1">
                <Pin className="h-3 w-3" />
                Pinned
              </span>
            )}
            {storedWallet?.probation_until &&
              new Date(storedWallet.probation_until) > new Date() && (
                <span className="px-2 py-1 rounded-full bg-orange-500/10 text-orange-600 text-xs font-medium flex items-center gap-1">
                  <AlertTriangle className="h-3 w-3" />
                  Probation
                </span>
              )}
            {storedWallet?.auto_assigned && (
              <span className="px-2 py-1 rounded-full bg-blue-500/10 text-blue-600 text-xs font-medium">
                Auto-selected
              </span>
            )}
            {(storedWallet?.consecutive_losses ?? 0) >= 3 && (
              <span className="px-2 py-1 rounded-full bg-loss/10 text-loss text-xs font-medium">
                {storedWallet!.consecutive_losses} losses
              </span>
            )}
            {discoveredWallet && (
              <>
                <StrategyBadge strategy={discoveredWallet.strategy_type} size="md" />
                <StalenessIndicator
                  stalenessDays={discoveredWallet.staleness_days ?? 0}
                  showWhenFresh
                />
                <CompositeScoreGauge
                  score={discoveredWallet.composite_score != null ? Number(discoveredWallet.composite_score) : undefined}
                />
              </>
            )}
            {/* Actions */}
            {isActive && (
              <Button
                variant="outline"
                onClick={handleDemote}
                disabled={demoteMutation.isPending}
                aria-label="Demote wallet"
              >
                {demoteMutation.isPending ? (
                  <Loader2 className="mr-1 h-4 w-4 animate-spin" />
                ) : (
                  <ChevronDown className="mr-1 h-4 w-4" />
                )}
                Demote
              </Button>
            )}
            {isBench && (
              <Button
                onClick={handlePromote}
                disabled={isRosterFull || promoteMutation.isPending}
                aria-label="Promote wallet"
              >
                {promoteMutation.isPending ? (
                  <Loader2 className="mr-1 h-4 w-4 animate-spin" />
                ) : (
                  <ChevronUp className="mr-1 h-4 w-4" />
                )}
                Promote
              </Button>
            )}
          </div>
        </div>

        {/* Stats Row */}
        <div className="grid gap-4 grid-cols-2 sm:grid-cols-3 lg:grid-cols-5">
          {/* ROI Card with period toggle */}
          <Card>
            <CardContent className="p-4">
              <div className="flex items-center gap-3">
                <TrendingUp className="h-5 w-5 text-profit" />
                <div className="flex-1">
                  <div className="flex items-center gap-1.5 mb-1">
                    {(["7d", "30d", "90d"] as RoiPeriod[]).map((period) => (
                      <button
                        key={period}
                        onClick={() => setRoiPeriod(period)}
                        className={`text-xs px-1.5 py-0.5 rounded transition-colors ${
                          roiPeriod === period
                            ? "bg-primary text-primary-foreground"
                            : "text-muted-foreground hover:bg-muted"
                        }`}
                      >
                        {period}
                      </button>
                    ))}
                  </div>
                  {isLoading ? (
                    <Skeleton className="h-6 w-16" />
                  ) : (
                    <p
                      className={`text-xl font-bold tabular-nums ${Number(selectedRoi) >= 0 ? "text-profit" : "text-loss"}`}
                    >
                      {Number(selectedRoi) >= 0 ? "+" : ""}
                      {Number(selectedRoi).toFixed(1)}%
                    </p>
                  )}
                </div>
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardContent className="p-4">
              <div className="flex items-center gap-3">
                <Target className="h-5 w-5 text-primary" />
                <div>
                  <p className="text-xs text-muted-foreground">Win Rate</p>
                  {isLoading ? (
                    <Skeleton className="h-6 w-16" />
                  ) : (
                    <p className="text-xl font-bold tabular-nums">
                      {Number(wallet.winRate).toFixed(1)}%
                    </p>
                  )}
                </div>
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardContent className="p-4">
              <div className="flex items-center gap-3">
                <Activity className="h-5 w-5 text-blue-500" />
                <div>
                  <p className="text-xs text-muted-foreground">Sharpe</p>
                  {isLoading ? (
                    <Skeleton className="h-6 w-12" />
                  ) : (
                    <p className="text-xl font-bold tabular-nums">
                      {Number(wallet.sharpe).toFixed(2)}
                    </p>
                  )}
                </div>
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardContent className="p-4">
              <div className="flex items-center gap-3">
                <TrendingDown className="h-5 w-5 text-loss" />
                <div>
                  <p className="text-xs text-muted-foreground">Max Drawdown</p>
                  {isLoading ? (
                    <Skeleton className="h-6 w-16" />
                  ) : (
                    <p className="text-xl font-bold text-loss tabular-nums">
                      {Number(wallet.maxDrawdown).toFixed(1)}%
                    </p>
                  )}
                </div>
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardContent className="p-4">
              <div className="flex items-center gap-3">
                <Zap className="h-5 w-5 text-yellow-500" />
                <div>
                  <p className="text-xs text-muted-foreground">Trades</p>
                  {isLoading ? (
                    <Skeleton className="h-6 w-12" />
                  ) : (
                    <p className="text-xl font-bold tabular-nums">
                      {wallet.trades}
                    </p>
                  )}
                </div>
              </div>
            </CardContent>
          </Card>
        </div>

        {/* Allocation Section - Show for active wallets (editable) and bench wallets (read-only) */}
        {(isActive || isBench) && (
          <WalletAllocationSection
            walletAddress={address}
            totalBalance={balance}
            positionsValue={walletPositionsValue}
            allocations={allocations}
            readOnly={isBench}
          />
        )}

        {/* Calibration + Copy Performance */}
        <div className="grid gap-4 md:grid-cols-2">
          <CalibrationChart />
          <CopyPerformance address={address} />
        </div>

        {/* Trade History */}
        <Card>
          <CardHeader>
            <CardTitle>Recent Trades</CardTitle>
          </CardHeader>
          <CardContent>
            {/* Trade summary stats */}
            {tradeSummary && (
              <div className="grid grid-cols-2 sm:grid-cols-4 gap-4 mb-4 p-3 bg-muted/30 rounded-lg border">
                <div className="text-center">
                  <p className="text-xs text-muted-foreground mb-1">
                    Total Volume
                  </p>
                  <p className="font-bold tabular-nums">
                    {formatCurrency(tradeSummary.totalVolume)}
                  </p>
                </div>
                <div className="text-center">
                  <p className="text-xs text-muted-foreground mb-1">
                    Avg Trade Size
                  </p>
                  <p className="font-bold tabular-nums">
                    {formatCurrency(tradeSummary.avgSize)}
                  </p>
                </div>
                <div className="text-center">
                  <p className="text-xs text-muted-foreground mb-1">Buys</p>
                  <p className="font-bold text-profit tabular-nums">
                    {tradeSummary.buys}
                  </p>
                </div>
                <div className="text-center">
                  <p className="text-xs text-muted-foreground mb-1">Sells</p>
                  <p className="font-bold text-loss tabular-nums">
                    {tradeSummary.sells}
                  </p>
                </div>
              </div>
            )}

            {isLoadingTrades ? (
              <div className="space-y-4">
                {[1, 2, 3].map((i) => (
                  <div key={i} className="flex items-center gap-4 p-4 border-b">
                    <div className="flex-1 space-y-2">
                      <Skeleton className="h-4 w-48" />
                      <Skeleton className="h-3 w-24" />
                    </div>
                    <Skeleton className="h-6 w-16" />
                    <Skeleton className="h-6 w-16" />
                  </div>
                ))}
              </div>
            ) : tradesError ? (
              <ErrorDisplay
                error={tradesError}
                onRetry={() => refetchTrades()}
                variant="card"
                title="Failed to load trades"
              />
            ) : walletTrades && walletTrades.length > 0 ? (
              <div className="space-y-0">
                <div className="overflow-x-auto">
                  <table className="w-full">
                    <thead className="border-b bg-muted/50">
                      <tr>
                        <th className="text-left p-4 font-medium">Market</th>
                        <th className="text-left p-4 font-medium">Side</th>
                        <th className="text-right p-4 font-medium">Price</th>
                        <th className="text-right p-4 font-medium">Value</th>
                        <th className="text-right p-4 font-medium">Time</th>
                      </tr>
                    </thead>
                    <tbody>
                      {walletTrades.map((trade) => (
                        <tr
                          key={trade.transaction_hash}
                          className="border-b hover:bg-muted/30"
                        >
                          <td className="p-4">
                            <p className="font-medium text-sm">
                              {trade.title || trade.asset_id}
                            </p>
                            <p className="text-xs text-muted-foreground">
                              {new Date(trade.timestamp).toLocaleDateString()}
                            </p>
                          </td>
                          <td className="p-4">
                            <span
                              className={`px-2 py-1 rounded text-xs font-medium uppercase ${
                                trade.side === "BUY"
                                  ? "bg-profit/10 text-profit"
                                  : "bg-loss/10 text-loss"
                              }`}
                            >
                              {trade.side}
                            </span>
                          </td>
                          <td className="p-4 text-right tabular-nums">
                            ${Number(trade.price).toFixed(2)}
                          </td>
                          <td className="p-4 text-right tabular-nums">
                            {formatCurrency(trade.value)}
                          </td>
                          <td className="p-4 text-right text-muted-foreground text-sm">
                            {new Date(trade.timestamp).toLocaleTimeString()}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                {walletTrades.length >= tradesLimit && (
                  <div className="flex justify-center pt-4">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => setTradesLimit((prev) => prev + 10)}
                    >
                      Load More
                    </Button>
                  </div>
                )}
              </div>
            ) : (
              <div className="py-12 text-center">
                <BarChart3 className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
                <h3 className="text-lg font-medium mb-2">No recent trades</h3>
                <p className="text-muted-foreground mb-4">
                  No trade history found for this wallet yet.
                </p>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => router.push("/discover")}
                >
                  <ShoppingCart className="mr-2 h-4 w-4" />
                  Discover Wallets
                </Button>
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </ErrorBoundary>
  );
}
