'use client';

import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { TrendingUp, TrendingDown, Activity, DollarSign } from 'lucide-react';
import { cn } from '@/lib/utils';
import type { OptimizerPortfolioMetrics } from '@/types/api';

interface PortfolioSummaryCardProps {
  metrics: OptimizerPortfolioMetrics | undefined;
  isLoading: boolean;
}

function formatCurrency(value: number): string {
  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    minimumFractionDigits: 0,
    maximumFractionDigits: 0,
  }).format(value);
}

function formatPercent(value: number): string {
  return `${value >= 0 ? '+' : ''}${value.toFixed(1)}%`;
}

export function PortfolioSummaryCard({ metrics, isLoading }: PortfolioSummaryCardProps) {
  if (isLoading) {
    return (
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <Card key={i}>
            <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
              <Skeleton className="h-4 w-24" />
              <Skeleton className="h-4 w-4" />
            </CardHeader>
            <CardContent>
              <Skeleton className="h-8 w-32 mb-1" />
              <Skeleton className="h-3 w-20" />
            </CardContent>
          </Card>
        ))}
      </div>
    );
  }

  const items = [
    {
      title: 'Total Value',
      value: formatCurrency(metrics?.total_value ?? 0),
      icon: DollarSign,
      description: 'Workspace budget',
    },
    {
      title: 'Portfolio ROI',
      value: formatPercent(metrics?.total_roi_30d ?? 0),
      icon: (metrics?.total_roi_30d ?? 0) >= 0 ? TrendingUp : TrendingDown,
      description: '30-day average',
      positive: (metrics?.total_roi_30d ?? 0) >= 0,
    },
    {
      title: 'Avg Sharpe',
      value: (metrics?.avg_sharpe ?? 0).toFixed(2),
      icon: Activity,
      description: 'Risk-adjusted return',
    },
    {
      title: 'Avg Win Rate',
      value: `${(metrics?.avg_win_rate ?? 0).toFixed(1)}%`,
      icon: TrendingUp,
      description: 'Across active wallets',
    },
  ];

  return (
    <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
      {items.map((item) => (
        <Card key={item.title}>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">
              {item.title}
            </CardTitle>
            <item.icon
              className={cn(
                'h-4 w-4',
                item.positive !== undefined
                  ? item.positive
                    ? 'text-profit'
                    : 'text-loss'
                  : 'text-muted-foreground'
              )}
            />
          </CardHeader>
          <CardContent>
            <div
              className={cn(
                'text-2xl font-bold',
                item.positive !== undefined
                  ? item.positive
                    ? 'text-profit'
                    : 'text-loss'
                  : ''
              )}
            >
              {item.value}
            </div>
            <p className="text-xs text-muted-foreground">{item.description}</p>
          </CardContent>
        </Card>
      ))}
    </div>
  );
}
