'use client';

import { useMemo } from 'react';
import { useParams } from 'next/navigation';
import Link from 'next/link';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { useToastStore } from '@/stores/toast-store';
import { useWalletQuery, useWalletMetricsQuery, useWalletBalanceQuery } from '@/hooks/queries/useWalletsQuery';
import { useLiveTradesQuery, useDiscoveredWalletQuery } from '@/hooks/queries/useDiscoverQuery';
import {
  useAllocationsQuery,
  useDemoteAllocationMutation,
  usePromoteAllocationMutation,
} from '@/hooks/queries/useAllocationsQuery';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { useModeStore } from '@/stores/mode-store';
import { shortenAddress, formatCurrency, ratioOrPercentToPercent } from '@/lib/utils';
import {
  ArrowLeft,
  Wallet,
  TrendingUp,
  TrendingDown,
  Shield,
  Target,
  CheckCircle,
  ChevronUp,
  ChevronDown,
  Zap,
  Activity,
  AlertCircle,
} from 'lucide-react';
import { WalletAllocationSection } from '@/components/trading/WalletAllocationSection';
import { useDemoPortfolioStore } from '@/stores/demo-portfolio-store';
import { useWalletStore } from '@/stores/wallet-store';
import type { TradingStyle, DecisionBrief, TradeClassification } from '@/types/api';

const tradingStyleLabels: Record<TradingStyle, string> = {
  event_trader: 'Event Trader',
  arb_trader: 'Arb Trader',
  mixed: 'Mixed Strategy',
};

const tradingStyleDescriptions: Record<TradingStyle, string> = {
  event_trader: 'Focuses on directional event trades with longer hold times',
  arb_trader: 'Primarily executes mathematical arbitrage opportunities',
  mixed: 'Combines event trading and arbitrage strategies',
};

const slippageColors = {
  tight: 'text-profit',
  moderate: 'text-yellow-500',
  loose: 'text-loss',
};

export default function WalletDetailPage() {
  const params = useParams();
  const address = params.address as string;
  const toast = useToastStore();
  const { currentWorkspace } = useWorkspaceStore();
  const { mode } = useModeStore();
  const { data: allocations = [] } = useAllocationsQuery(currentWorkspace?.id, mode);
  const promoteMutation = usePromoteAllocationMutation(currentWorkspace?.id, mode);
  const demoteMutation = useDemoteAllocationMutation(currentWorkspace?.id, mode);
  const { balance: demoBalance, positions } = useDemoPortfolioStore();
  const { primaryWallet } = useWalletStore();
  const { data: walletBalance } = useWalletBalanceQuery(
    mode === 'live' ? primaryWallet : null
  );
  const balance = mode === 'live' && walletBalance ? walletBalance.usdc_balance : demoBalance;

  // Fetch wallet data from API
  const { data: apiWallet, isLoading: isLoadingWallet, error: walletError } = useWalletQuery(mode, address);
  const { data: walletMetrics, isLoading: isLoadingMetrics } = useWalletMetricsQuery(mode, address);
  // Fallback to discovery data when wallet isn't tracked
  const { data: discoveredWallet, isLoading: isLoadingDiscovered } = useDiscoveredWalletQuery(mode, address);
  const { data: recentTrades, isLoading: isLoadingTrades } = useLiveTradesQuery(mode, {
    wallet: address,
    limit: 10,
    workspaceId: currentWorkspace?.id,
  });

  // Find wallet in workspace allocations
  const storedWallet = useMemo(() => {
    return allocations.find((w) => w.wallet_address.toLowerCase() === address?.toLowerCase());
  }, [allocations, address]);

  // Merge API data with stored wallet data
  const wallet = useMemo(() => {
    if (storedWallet) {
      // Use backtest data if available, otherwise fall back to discovery data
      const hasBacktest = storedWallet.backtest_roi != null && storedWallet.backtest_roi !== 0;
      return {
        address: storedWallet.wallet_address,
        label: storedWallet.wallet_label,
        tier: storedWallet.tier,
        roi30d: hasBacktest
          ? ratioOrPercentToPercent(storedWallet.backtest_roi)
          : (discoveredWallet ? Number(discoveredWallet.roi_30d) : 0),
        roi7d: discoveredWallet ? Number(discoveredWallet.roi_7d) : 0,
        roi90d: discoveredWallet ? Number(discoveredWallet.roi_90d) : 0,
        sharpe: storedWallet.backtest_sharpe ?? (discoveredWallet ? Number(discoveredWallet.sharpe_ratio) : 0),
        winRate: hasBacktest
          ? ratioOrPercentToPercent(storedWallet.backtest_win_rate)
          : (discoveredWallet ? Number(discoveredWallet.win_rate) : 0),
        trades: discoveredWallet?.total_trades ?? 0,
        maxDrawdown: discoveredWallet ? Number(discoveredWallet.max_drawdown) : 0,
        confidence: storedWallet.confidence_score ?? (discoveredWallet?.confidence ?? 0),
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
        tier: apiWallet.copy_enabled ? 'active' as const : 'bench' as const,
        roi30d: ratioOrPercentToPercent(walletMetrics?.roi),
        roi7d: 0, // Not available in WalletMetrics
        roi90d: 0, // Not available in WalletMetrics
        sharpe: walletMetrics?.sharpe_ratio ?? 0,
        winRate: ratioOrPercentToPercent(apiWallet?.win_rate),
        trades: apiWallet?.total_trades ?? 0,
        maxDrawdown: ratioOrPercentToPercent(walletMetrics?.max_drawdown),
        confidence: 0,
        copySettings: {
          copy_behavior: 'events_only' as const,
          allocation_pct: apiWallet.allocation_pct ?? 0,
          max_position_size: apiWallet.max_position_size ?? 100,
        },
        addedAt: apiWallet.added_at ?? new Date().toISOString(),
      };
    }
    // Fallback to discovery data for untracked wallets
    if (discoveredWallet) {
      return {
        address: discoveredWallet.address,
        label: undefined,
        tier: 'bench' as const,
        roi30d: Number(discoveredWallet.roi_30d),
        roi7d: Number(discoveredWallet.roi_7d),
        roi90d: Number(discoveredWallet.roi_90d),
        sharpe: Number(discoveredWallet.sharpe_ratio),
        winRate: Number(discoveredWallet.win_rate),
        trades: discoveredWallet.total_trades,
        maxDrawdown: Number(discoveredWallet.max_drawdown),
        confidence: discoveredWallet.confidence,
        copySettings: {
          copy_behavior: 'events_only' as const,
          allocation_pct: 0,
          max_position_size: 100,
        },
        addedAt: new Date().toISOString(),
      };
    }
    // Return minimal wallet for display while loading
    return {
      address: address,
      label: undefined,
      tier: 'bench' as const,
      roi30d: 0,
      sharpe: 0,
      winRate: 0,
      trades: 0,
      maxDrawdown: 0,
      confidence: 0,
      copySettings: {
        copy_behavior: 'events_only' as const,
        allocation_pct: 0,
        max_position_size: 100,
      },
      addedAt: new Date().toISOString(),
    };
  }, [storedWallet, apiWallet, walletMetrics, discoveredWallet, address]);

  const decisionBrief = undefined as DecisionBrief | undefined;

  // Calculate positions value for this wallet
  const walletPositionsValue = useMemo(() => {
    return positions
      .filter((p) => p.walletAddress?.toLowerCase() === address?.toLowerCase())
      .reduce((sum, p) => sum + (p.entryPrice * p.quantity), 0);
  }, [positions, address]);

  const isActive = allocations.some((w) => w.wallet_address.toLowerCase() === address?.toLowerCase() && w.tier === 'active');
  const isBench = allocations.some((w) => w.wallet_address.toLowerCase() === address?.toLowerCase() && w.tier === 'bench');
  const isLoading = isLoadingWallet || isLoadingMetrics || isLoadingDiscovered;
  const isRosterFull = () => allocations.filter((a) => a.tier === 'active').length >= 5;

  const handlePromote = () => {
    if (isRosterFull()) {
      toast.error('Roster Full', 'Demote a wallet first to make room');
      return;
    }
    promoteMutation.mutate(address, {
      onSuccess: () => toast.success('Promoted!', `${shortenAddress(address)} added to Active`),
      onError: () => toast.error('Promotion Failed', 'Could not promote wallet'),
    });
  };

  const handleDemote = () => {
    demoteMutation.mutate(address, {
      onSuccess: () => toast.info('Demoted', `${shortenAddress(address)} moved to Bench`),
      onError: () => toast.error('Demotion Failed', 'Could not demote wallet'),
    });
  };

  return (
    <div className="space-y-6">
      {/* Breadcrumb & Header */}
      <div className="flex items-center gap-4">
        <Link href="/trading">
          <Button variant="ghost" size="icon">
            <ArrowLeft className="h-5 w-5" />
          </Button>
        </Link>
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
                  <p className="text-muted-foreground font-mono">{shortenAddress(address)}</p>
                </>
              )}
            </div>
          </div>
        </div>
        <div className="flex items-center gap-2">
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
          {isActive && (
            <Button variant="outline" onClick={handleDemote}>
              <ChevronDown className="mr-1 h-4 w-4" />
              Demote
            </Button>
          )}
          {isBench && (
            <Button onClick={handlePromote} disabled={isRosterFull()}>
              <ChevronUp className="mr-1 h-4 w-4" />
              Promote
            </Button>
          )}
        </div>
      </div>

      {/* Stats Row */}
      <div className="grid gap-4 md:grid-cols-5">
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <TrendingUp className="h-5 w-5 text-profit" />
              <div>
                <p className="text-xs text-muted-foreground">ROI (30d)</p>
                {isLoading ? (
                  <Skeleton className="h-6 w-16" />
                ) : (
                  <p className={`text-xl font-bold ${Number(wallet.roi30d) >= 0 ? 'text-profit' : 'text-loss'}`}>
                    {Number(wallet.roi30d) >= 0 ? '+' : ''}{Number(wallet.roi30d).toFixed(1)}%
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
                  <p className="text-xl font-bold">{Number(wallet.winRate).toFixed(1)}%</p>
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
                  <p className="text-xl font-bold">{Number(wallet.sharpe).toFixed(2)}</p>
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
                  <p className="text-xl font-bold text-loss">{Number(wallet.maxDrawdown).toFixed(1)}%</p>
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
                  <p className="text-xl font-bold">{wallet.trades}</p>
                )}
              </div>
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Allocation Section - Only show for active wallets */}
      {isActive && (
        <WalletAllocationSection
          walletAddress={address}
          totalBalance={balance}
          positionsValue={walletPositionsValue}
          isDemo={mode === 'demo'}
        />
      )}

      {/* Decision Brief - Only show if data is available */}
      {decisionBrief && (
        <Card className="border-primary/50">
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Shield className="h-5 w-5" />
              Decision Brief
            </CardTitle>
            <CardDescription>
              Strategy profile and fitness assessment for copy trading
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            {/* Trading Style */}
            <div className="grid md:grid-cols-2 gap-6">
              <div className="space-y-4">
                <div>
                  <p className="text-sm text-muted-foreground mb-1">Trading Style</p>
                  <p className="text-lg font-semibold">
                    {tradingStyleLabels[decisionBrief.trading_style]}
                  </p>
                  <p className="text-sm text-muted-foreground">
                    {tradingStyleDescriptions[decisionBrief.trading_style]}
                  </p>
                </div>

                <div className="grid grid-cols-2 gap-4">
                  <div>
                    <p className="text-sm text-muted-foreground mb-1">Event Trades</p>
                    <p className="text-lg font-semibold">
                      {Math.round(decisionBrief.event_trade_ratio * 100)}%
                    </p>
                  </div>
                  <div>
                    <p className="text-sm text-muted-foreground mb-1">Arb Trades</p>
                    <p className="text-lg font-semibold">
                      {Math.round(decisionBrief.arb_trade_ratio * 100)}%
                    </p>
                  </div>
                </div>

                <div>
                  <p className="text-sm text-muted-foreground mb-1">Slippage Tolerance</p>
                  <p className={`text-lg font-semibold capitalize ${slippageColors[decisionBrief.slippage_tolerance]}`}>
                    {decisionBrief.slippage_tolerance}
                  </p>
                </div>

                <div>
                  <p className="text-sm text-muted-foreground mb-1">Typical Hold Time</p>
                  <p className="text-lg font-semibold">{decisionBrief.typical_hold_time}</p>
                </div>
              </div>

              <div className="space-y-4">
                <div>
                  <p className="text-sm text-muted-foreground mb-1">Preferred Categories</p>
                  <div className="flex flex-wrap gap-2 mt-2">
                    {decisionBrief.preferred_categories.map((cat) => (
                      <span
                        key={cat}
                        className="px-3 py-1 rounded-full bg-primary/10 text-primary text-sm"
                      >
                        {cat}
                      </span>
                    ))}
                  </div>
                </div>

                <div>
                  <p className="text-sm text-muted-foreground mb-2">Fitness Score</p>
                  <div className="flex items-center gap-3">
                    <div className="flex-1 h-3 bg-muted rounded-full overflow-hidden">
                      <div
                        className="h-full bg-profit rounded-full transition-all"
                        style={{ width: `${decisionBrief.fitness_score}%` }}
                      />
                    </div>
                    <span className="text-lg font-bold">{decisionBrief.fitness_score}/100</span>
                  </div>
                </div>
              </div>
            </div>

            {/* Fitness Reasons */}
            <div>
              <p className="text-sm text-muted-foreground mb-3 font-medium uppercase">
                Assessment
              </p>
              <ul className="space-y-2">
                {decisionBrief.fitness_reasons.map((reason, i) => (
                  <li key={i} className="flex items-start gap-2 text-sm">
                    <CheckCircle className="h-4 w-4 text-profit mt-0.5 shrink-0" />
                    {reason}
                  </li>
                ))}
              </ul>
            </div>
          </CardContent>
        </Card>
      )}

      {/* No Decision Brief Placeholder */}
      {!decisionBrief && !isLoading && (
        <Card className="border-muted">
          <CardContent className="p-8 text-center">
            <AlertCircle className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
            <h3 className="text-lg font-medium mb-2">No Decision Brief Available</h3>
            <p className="text-muted-foreground">
              Strategy profile data is not yet available for this wallet.
            </p>
          </CardContent>
        </Card>
      )}

      {/* Trade History */}
      <Card>
        <CardHeader>
          <CardTitle>Recent Trades</CardTitle>
        </CardHeader>
        <CardContent>
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
          ) : recentTrades && recentTrades.length > 0 ? (
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
                  {recentTrades.map((trade) => (
                    <tr key={trade.tx_hash} className="border-b hover:bg-muted/30">
                      <td className="p-4">
                        <p className="font-medium text-sm">{trade.market_question || trade.market_id}</p>
                        <p className="text-xs text-muted-foreground">
                          {new Date(trade.timestamp).toLocaleDateString()}
                        </p>
                      </td>
                      <td className="p-4">
                        <span
                          className={`px-2 py-1 rounded text-xs font-medium uppercase ${
                            trade.direction === 'buy'
                              ? 'bg-profit/10 text-profit'
                              : 'bg-loss/10 text-loss'
                          }`}
                        >
                          {trade.direction}
                        </span>
                      </td>
                      <td className="p-4 text-right tabular-nums">${Number(trade.price).toFixed(2)}</td>
                      <td className="p-4 text-right tabular-nums">{formatCurrency(trade.value)}</td>
                      <td className="p-4 text-right text-muted-foreground text-sm">
                        {new Date(trade.timestamp).toLocaleTimeString()}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <div className="py-12 text-center">
              <p className="text-muted-foreground">
                No recent trades found for this wallet.
              </p>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
