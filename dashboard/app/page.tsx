'use client';

import { useState } from 'react';
import { usePortfolioStats } from '@/hooks/usePortfolioStats';
import { useActivity } from '@/hooks/useActivity';
import { useWalletsQuery, useRecommendationsQuery } from '@/hooks/queries/useWalletsQuery';
import { useToastStore } from '@/stores/toast-store';
import { MetricCard } from '@/components/shared/MetricCard';
import { ConnectionStatus } from '@/components/shared/ConnectionStatus';
import { LiveIndicator } from '@/components/shared/LiveIndicator';
import { PortfolioChart } from '@/components/charts/PortfolioChart';
import { AllocationPie } from '@/components/charts/AllocationPie';
import { CopyWalletModal } from '@/components/modals/CopyWalletModal';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import {
  TrendingDown,
  Activity,
  Wallet,
  ArrowRight,
  Copy,
  Zap,
  AlertCircle,
} from 'lucide-react';
import Link from 'next/link';
import { formatCurrency, shortenAddress, formatTimeAgo } from '@/lib/utils';
import { cn } from '@/lib/utils';
import type { CopyBehavior } from '@/types/api';

type Period = '1D' | '7D' | '30D' | 'ALL';

const activityIcons: Record<string, React.ReactNode> = {
  TRADE_COPIED: <Copy className="h-4 w-4 text-blue-500" />,
  STOP_LOSS_TRIGGERED: <TrendingDown className="h-4 w-4 text-loss" />,
  RECOMMENDATION_NEW: <Activity className="h-4 w-4 text-purple-500" />,
  ARBITRAGE_DETECTED: <Zap className="h-4 w-4 text-yellow-500" />,
  POSITION_OPENED: <AlertCircle className="h-4 w-4 text-profit" />,
  POSITION_CLOSED: <AlertCircle className="h-4 w-4 text-muted-foreground" />,
};

interface SelectedWallet {
  address: string;
  roi30d?: number;
  sharpe?: number;
  winRate?: number;
  trades?: number;
  confidence?: number;
}

export default function DashboardPage() {
  const toast = useToastStore();
  const [selectedPeriod, setSelectedPeriod] = useState<Period>('30D');
  const [copyModalOpen, setCopyModalOpen] = useState(false);
  const [selectedWallet, setSelectedWallet] = useState<SelectedWallet | null>(null);

  // Real-time data hooks
  const { stats, history, status: portfolioStatus } = usePortfolioStats(selectedPeriod);
  const { activities, status: activityStatus, unreadCount } = useActivity();

  // Query hooks for wallets and recommendations
  const { data: wallets = [] } = useWalletsQuery({ copyEnabled: true });
  const { data: recommendations = [] } = useRecommendationsQuery();

  // Compute roster count from wallets with copy_enabled
  const rosterCount = wallets.filter(w => w.copy_enabled).length;

  // Build allocations from active wallets
  const COLORS = ['#3b82f6', '#22c55e', '#a855f7', '#f97316', '#ec4899'];
  const allocations = wallets.slice(0, 5).map((w, i) => ({
    name: shortenAddress(w.address),
    value: w.allocation_pct || 20,
    color: COLORS[i % COLORS.length],
  }));

  const handleCopyClick = (wallet: SelectedWallet) => {
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
    // In real app, this would call the API
    const tierLabel = settings.tier === 'active' ? 'Active 5' : 'Bench';
    toast.success(
      `Wallet added to ${tierLabel}`,
      `${shortenAddress(settings.address)} is now being ${settings.tier === 'active' ? 'copied' : 'monitored'}`
    );
    setCopyModalOpen(false);
    setSelectedWallet(null);
  };

  // Convert history to chart format
  const chartData = history.map((h) => ({
    time: h.timestamp.split('T')[0],
    value: h.value,
  }));

  return (
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-4">
          <div>
            <h1 className="text-3xl font-bold tracking-tight">Dashboard</h1>
            <p className="text-muted-foreground">
              Live trading dashboard
            </p>
          </div>
          <LiveIndicator />
          <ConnectionStatus status={portfolioStatus} showLabel />
        </div>
        <Link href="/allocate">
          <Button>
            <Sliders className="mr-2 h-4 w-4" />
            Allocate Capital
          </Button>
        </Link>
      </div>

      {/* Stats Grid */}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <MetricCard
          title="Total Value"
          value={formatCurrency(stats.total_value)}
          change={stats.total_pnl_percent}
          trend={stats.total_pnl_percent >= 0 ? 'up' : 'down'}
        />
        <MetricCard
          title="Today's P&L"
          value={formatCurrency(stats.today_pnl, { showSign: true })}
          change={stats.today_pnl_percent}
          trend={stats.today_pnl >= 0 ? 'up' : 'down'}
        />
        <MetricCard
          title="Win Rate"
          value={`${stats.win_rate}%`}
          changeLabel={`${stats.winning_trades}/${stats.total_trades} trades`}
          trend="neutral"
        />
        <MetricCard
          title="Active Positions"
          value={stats.active_positions.toString()}
          changeLabel="Open positions"
          trend="neutral"
        />
      </div>

      {/* Charts Row */}
      <div className="grid gap-6 lg:grid-cols-3">
        {/* Portfolio Value Chart */}
        <Card className="lg:col-span-2">
          <CardHeader className="flex flex-row items-center justify-between">
            <div className="flex items-center gap-2">
              <CardTitle>Portfolio Value</CardTitle>
              <LiveIndicator label="" />
            </div>
            <div className="flex gap-1">
              {(['1D', '7D', '30D', 'ALL'] as Period[]).map((period) => (
                <Button
                  key={period}
                  variant={selectedPeriod === period ? 'default' : 'ghost'}
                  size="sm"
                  onClick={() => setSelectedPeriod(period)}
                >
                  {period}
                </Button>
              ))}
            </div>
          </CardHeader>
          <CardContent>
            <PortfolioChart data={chartData} height={300} />
          </CardContent>
        </Card>

        {/* Allocation Pie */}
        <Card>
          <CardHeader>
            <CardTitle>Strategy Allocation</CardTitle>
          </CardHeader>
          <CardContent>
            <AllocationPie data={allocations.length > 0 ? allocations : [{ name: 'No allocations', value: 100, color: '#6b7280' }]} />
            <Link href="/allocate">
              <Button variant="outline" className="w-full mt-4">
                Manage Allocations
                <ArrowRight className="ml-2 h-4 w-4" />
              </Button>
            </Link>
          </CardContent>
        </Card>
      </div>

      {/* Activity & Recommendations */}
      <div className="grid gap-6 lg:grid-cols-2">
        {/* Recent Activity - Now Real-time */}
        <Card>
          <CardHeader className="flex flex-row items-center justify-between">
            <div className="flex items-center gap-2">
              <CardTitle>Recent Activity</CardTitle>
              {unreadCount > 0 && (
                <span className="flex h-5 min-w-5 items-center justify-center rounded-full bg-primary px-1.5 text-xs font-medium text-primary-foreground">
                  {unreadCount}
                </span>
              )}
              <ConnectionStatus status={activityStatus} />
            </div>
            <Link href="/portfolio">
              <Button variant="ghost" size="sm">
                View All
                <ArrowRight className="ml-2 h-4 w-4" />
              </Button>
            </Link>
          </CardHeader>
          <CardContent>
            <div className="space-y-4 max-h-[400px] overflow-y-auto">
              {activities.slice(0, 10).map((item, index) => (
                <div
                  key={item.id}
                  className={cn(
                    'flex items-start gap-3',
                    index === 0 && 'animate-slide-in'
                  )}
                >
                  <div className="mt-1">
                    {activityIcons[item.type] || <Activity className="h-4 w-4" />}
                  </div>
                  <div className="flex-1 space-y-1">
                    <p className="text-sm">{item.message}</p>
                    {item.details && (
                      <p className="text-xs text-muted-foreground">
                        {Object.entries(item.details)
                          .map(([k, v]) => `${k}: ${v}`)
                          .join(' | ')}
                      </p>
                    )}
                    <p className="text-xs text-muted-foreground">
                      {formatTimeAgo(item.created_at)}
                    </p>
                  </div>
                  {item.pnl !== undefined && (
                    <span
                      className={cn(
                        'text-sm font-medium tabular-nums',
                        item.pnl >= 0 ? 'text-profit' : 'text-loss'
                      )}
                    >
                      {item.pnl >= 0 ? '+' : ''}
                      {formatCurrency(item.pnl)}
                    </span>
                  )}
                </div>
              ))}
            </div>
          </CardContent>
        </Card>

        {/* Top Recommendations */}
        <Card>
          <CardHeader className="flex flex-row items-center justify-between">
            <CardTitle>Top Recommendations</CardTitle>
            <Link href="/discover">
              <Button variant="ghost" size="sm">
                View All
                <ArrowRight className="ml-2 h-4 w-4" />
              </Button>
            </Link>
          </CardHeader>
          <CardContent>
            <div className="space-y-4">
              {recommendations.length === 0 ? (
                <p className="text-sm text-muted-foreground text-center py-4">
                  No recommendations available. Check the Discover page for top wallets.
                </p>
              ) : (
                recommendations.slice(0, 2).map((rec) => (
                  <div
                    key={rec.address}
                    className="rounded-lg border p-4 space-y-3 hover:border-primary transition-colors"
                  >
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-2">
                        <Wallet className="h-4 w-4" />
                        <span className="font-medium">
                          {shortenAddress(rec.address)}
                        </span>
                      </div>
                      <span className="text-sm bg-live/10 text-live px-2 py-1 rounded-full">
                        {rec.confidence}% confidence
                      </span>
                    </div>
                    <div className="grid grid-cols-3 gap-4 text-sm">
                      <div>
                        <p className="text-muted-foreground">ROI</p>
                        <p className="font-medium text-profit">+{Number(rec.roi_30d || 0).toFixed(1)}%</p>
                      </div>
                      <div>
                        <p className="text-muted-foreground">Sharpe</p>
                        <p className="font-medium">{Number(rec.sharpe_ratio || 0).toFixed(2)}</p>
                      </div>
                      <div>
                        <p className="text-muted-foreground">Win Rate</p>
                        <p className="font-medium">{Number(rec.win_rate || 0).toFixed(1)}%</p>
                      </div>
                    </div>
                    <Button
                      className="w-full"
                      variant="default"
                      size="sm"
                      onClick={() =>
                        handleCopyClick({
                          address: rec.address,
                          roi30d: rec.roi_30d,
                          sharpe: rec.sharpe_ratio,
                          winRate: rec.win_rate,
                          confidence: rec.confidence,
                        })
                      }
                    >
                      <Copy className="mr-2 h-4 w-4" />
                      Copy Wallet
                    </Button>
                  </div>
                ))
              )}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Copy Wallet Modal */}
      <CopyWalletModal
        wallet={selectedWallet}
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

// Import for the button in header
function Sliders(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="24"
      height="24"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      {...props}
    >
      <line x1="4" x2="4" y1="21" y2="14" />
      <line x1="4" x2="4" y1="10" y2="3" />
      <line x1="12" x2="12" y1="21" y2="12" />
      <line x1="12" x2="12" y1="8" y2="3" />
      <line x1="20" x2="20" y1="21" y2="16" />
      <line x1="20" x2="20" y1="12" y2="3" />
      <line x1="2" x2="6" y1="14" y2="14" />
      <line x1="10" x2="14" y1="8" y2="8" />
      <line x1="18" x2="22" y1="16" y2="16" />
    </svg>
  );
}
