'use client';

import { useState } from 'react';
import Link from 'next/link';
import { usePortfolioStats } from '@/hooks/usePortfolioStats';
import { useActivity } from '@/hooks/useActivity';
import { useRosterStore } from '@/stores/roster-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { MetricCard } from '@/components/shared/MetricCard';
import { ConnectionStatus } from '@/components/shared/ConnectionStatus';
import { LiveIndicator } from '@/components/shared/LiveIndicator';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import {
  Activity,
  ArrowRight,
  Copy,
  TrendingDown,
  Zap,
  AlertCircle,
  XCircle,
  DollarSign,
  CheckCircle2,
  ShieldAlert,
  Search,
  PieChart,
  TrendingUp,
  Target,
  Settings2,
  Star,
} from 'lucide-react';
import { formatCurrency, formatTimeAgo } from '@/lib/utils';
import { cn } from '@/lib/utils';

type Period = '1D' | '7D' | '30D' | 'ALL';

const activityIcons: Record<string, React.ReactNode> = {
  TRADE_COPIED: <Copy className="h-4 w-4 text-blue-500" />,
  TRADE_COPY_SKIPPED: <AlertCircle className="h-4 w-4 text-yellow-500" />,
  TRADE_COPY_FAILED: <XCircle className="h-4 w-4 text-red-500" />,
  STOP_LOSS_TRIGGERED: <TrendingDown className="h-4 w-4 text-loss" />,
  RECOMMENDATION_NEW: <Activity className="h-4 w-4 text-purple-500" />,
  ARBITRAGE_DETECTED: <Zap className="h-4 w-4 text-yellow-500" />,
  ARB_POSITION_OPENED: <DollarSign className="h-4 w-4 text-profit" />,
  ARB_POSITION_CLOSED: <CheckCircle2 className="h-4 w-4 text-blue-500" />,
  ARB_EXECUTION_FAILED: <XCircle className="h-4 w-4 text-red-500" />,
  ARB_EXIT_FAILED: <ShieldAlert className="h-4 w-4 text-red-400" />,
  POSITION_OPENED: <AlertCircle className="h-4 w-4 text-profit" />,
  POSITION_CLOSED: <AlertCircle className="h-4 w-4 text-muted-foreground" />,
};

export function DashboardHome() {
  const [selectedPeriod, setSelectedPeriod] = useState<Period>('30D');
  const { currentWorkspace } = useWorkspaceStore();
  const { activeWallets } = useRosterStore();
  const { stats, status: portfolioStatus } = usePortfolioStats(selectedPeriod);
  const { activities, status: activityStatus, unreadCount } = useActivity();

  const isAutomatic = currentWorkspace?.setup_mode === 'automatic';
  const modeLabel = isAutomatic ? 'Guided' : 'Custom';
  const ModeIcon = isAutomatic ? Target : Settings2;

  return (
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-4">
          <div>
            <h1 className="text-3xl font-bold tracking-tight">Dashboard</h1>
            <p className="text-muted-foreground">
              Monitor your portfolio and trading activity
            </p>
          </div>
          <LiveIndicator />
          <ConnectionStatus status={portfolioStatus} showLabel />
        </div>
        <Badge variant="secondary" className="flex items-center gap-1">
          <ModeIcon className="h-3 w-3" />
          {modeLabel} Mode
        </Badge>
      </div>

      {/* Stats Grid */}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <MetricCard
          title="Portfolio Value"
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
          title="Active Wallets"
          value={`${activeWallets.length}/5`}
          changeLabel={activeWallets.length < 5 ? `${5 - activeWallets.length} slots available` : 'Roster full'}
          trend="neutral"
        />
        <MetricCard
          title="Open Positions"
          value={stats.active_positions.toString()}
          changeLabel={`Win rate: ${stats.win_rate}%`}
          trend="neutral"
        />
      </div>

      {/* Main Content */}
      <div className="grid gap-6 lg:grid-cols-2">
        {/* Recent Activity */}
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
            <div className="space-y-4 max-h-[350px] overflow-y-auto">
              {activities.length === 0 ? (
                <p className="text-sm text-muted-foreground text-center py-8">
                  No recent activity yet. Activity will appear here when wallets make trades.
                </p>
              ) : (
                activities.slice(0, 8).map((item, index) => (
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
                ))
              )}
            </div>
          </CardContent>
        </Card>

        {/* Quick Actions */}
        <Card>
          <CardHeader>
            <CardTitle>Quick Actions</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <p className="text-sm text-muted-foreground">
              Common actions to manage your portfolio
            </p>

            <div className="grid gap-3">
              <Link href="/discover">
                <Button variant="outline" className="w-full justify-start h-auto py-3">
                  <div className="flex items-center gap-3">
                    <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-blue-500/10">
                      <Search className="h-5 w-5 text-blue-500" />
                    </div>
                    <div className="text-left">
                      <p className="font-medium">Discover Wallets</p>
                      <p className="text-xs text-muted-foreground">
                        Find top performers to copy
                      </p>
                    </div>
                  </div>
                </Button>
              </Link>

              <Link href="/portfolio">
                <Button variant="outline" className="w-full justify-start h-auto py-3">
                  <div className="flex items-center gap-3">
                    <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-green-500/10">
                      <PieChart className="h-5 w-5 text-green-500" />
                    </div>
                    <div className="text-left">
                      <p className="font-medium">View Positions</p>
                      <p className="text-xs text-muted-foreground">
                        {stats.active_positions} open position{stats.active_positions !== 1 ? 's' : ''}
                      </p>
                    </div>
                  </div>
                </Button>
              </Link>

              <Link href="/backtest">
                <Button variant="outline" className="w-full justify-start h-auto py-3">
                  <div className="flex items-center gap-3">
                    <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-purple-500/10">
                      <TrendingUp className="h-5 w-5 text-purple-500" />
                    </div>
                    <div className="text-left">
                      <p className="font-medium">Run Backtest</p>
                      <p className="text-xs text-muted-foreground">
                        Test strategies on historical data
                      </p>
                    </div>
                  </div>
                </Button>
              </Link>

              <Link href="/roster">
                <Button variant="outline" className="w-full justify-start h-auto py-3">
                  <div className="flex items-center gap-3">
                    <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-yellow-500/10">
                      <Star className="h-5 w-5 text-yellow-500" />
                    </div>
                    <div className="text-left">
                      <p className="font-medium">Manage Active Wallets</p>
                      <p className="text-xs text-muted-foreground">
                        {activeWallets.length}/5 slots filled
                      </p>
                    </div>
                  </div>
                </Button>
              </Link>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
