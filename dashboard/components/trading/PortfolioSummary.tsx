'use client';

import { memo } from 'react';
import { Card, CardContent } from '@/components/ui/card';
import { TrendingUp, TrendingDown, PieChart, Trophy } from 'lucide-react';
import { formatCurrency, cn } from '@/lib/utils';

interface PortfolioSummaryProps {
  totalValue: number;
  totalPnl: number;
  positionCount: number;
  winRate: number;
  isDemo?: boolean;
}

export const PortfolioSummary = memo(function PortfolioSummary({
  totalValue,
  totalPnl,
  positionCount,
  winRate,
  isDemo = false,
}: PortfolioSummaryProps) {
  const pnlPercent = totalValue > 0 ? (totalPnl / (totalValue - totalPnl)) * 100 : 0;

  return (
    <div className="grid gap-4 md:grid-cols-4">
      <Card>
        <CardContent className="p-4">
          <div className="flex items-center gap-3">
            <div className="h-10 w-10 rounded-full bg-primary/10 flex items-center justify-center">
              <PieChart className="h-5 w-5 text-primary" />
            </div>
            <div>
              <p className="text-sm text-muted-foreground">
                {isDemo ? 'Demo Value' : 'Total Value'}
              </p>
              <p className="text-2xl font-bold tabular-nums">
                {formatCurrency(totalValue)}
              </p>
            </div>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardContent className="p-4">
          <div className="flex items-center gap-3">
            <div
              className={cn(
                'h-10 w-10 rounded-full flex items-center justify-center',
                totalPnl >= 0 ? 'bg-profit/10' : 'bg-loss/10'
              )}
            >
              {totalPnl >= 0 ? (
                <TrendingUp className="h-5 w-5 text-profit" />
              ) : (
                <TrendingDown className="h-5 w-5 text-loss" />
              )}
            </div>
            <div>
              <p className="text-sm text-muted-foreground">P&L</p>
              <p
                className={cn(
                  'text-2xl font-bold tabular-nums',
                  totalPnl >= 0 ? 'text-profit' : 'text-loss'
                )}
              >
                {formatCurrency(totalPnl, { showSign: true })}
              </p>
            </div>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardContent className="p-4">
          <div className="flex items-center gap-3">
            <div className="h-10 w-10 rounded-full bg-muted flex items-center justify-center">
              <PieChart className="h-5 w-5 text-muted-foreground" />
            </div>
            <div>
              <p className="text-sm text-muted-foreground">Open Positions</p>
              <p className="text-2xl font-bold tabular-nums">{positionCount}</p>
            </div>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardContent className="p-4">
          <div className="flex items-center gap-3">
            <div
              className={cn(
                'h-10 w-10 rounded-full flex items-center justify-center',
                winRate >= 50 ? 'bg-profit/10' : 'bg-loss/10'
              )}
            >
              <Trophy
                className={cn(
                  'h-5 w-5',
                  winRate >= 50 ? 'text-profit' : 'text-loss'
                )}
              />
            </div>
            <div>
              <p className="text-sm text-muted-foreground">Win Rate</p>
              <p className="text-2xl font-bold tabular-nums">{winRate.toFixed(0)}%</p>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>
  );
});
