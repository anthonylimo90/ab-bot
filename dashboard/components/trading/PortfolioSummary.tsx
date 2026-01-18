'use client';

import { memo } from 'react';
import { Card, CardContent } from '@/components/ui/card';
import { TrendingUp, TrendingDown, PieChart, Trophy, Wallet, CheckCircle } from 'lucide-react';
import { formatCurrency, cn } from '@/lib/utils';

interface PortfolioSummaryProps {
  totalValue: number;
  totalPnl: number;
  positionCount: number;
  winRate: number;
  realizedPnl?: number;
  availableBalance?: number;
  isDemo?: boolean;
}

export const PortfolioSummary = memo(function PortfolioSummary({
  totalValue,
  totalPnl,
  positionCount,
  winRate,
  realizedPnl = 0,
  availableBalance,
  isDemo = false,
}: PortfolioSummaryProps) {
  const unrealizedPnl = totalPnl - realizedPnl;
  const pnlPercent = totalValue > 0 ? (totalPnl / (totalValue - totalPnl)) * 100 : 0;

  return (
    <div className="grid gap-4 md:grid-cols-3 lg:grid-cols-6">
      {/* Total Value */}
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

      {/* Unrealized P&L */}
      <Card>
        <CardContent className="p-4">
          <div className="flex items-center gap-3">
            <div
              className={cn(
                'h-10 w-10 rounded-full flex items-center justify-center',
                unrealizedPnl >= 0 ? 'bg-profit/10' : 'bg-loss/10'
              )}
            >
              {unrealizedPnl >= 0 ? (
                <TrendingUp className="h-5 w-5 text-profit" />
              ) : (
                <TrendingDown className="h-5 w-5 text-loss" />
              )}
            </div>
            <div>
              <p className="text-sm text-muted-foreground">Unrealized P&L</p>
              <p
                className={cn(
                  'text-2xl font-bold tabular-nums',
                  unrealizedPnl >= 0 ? 'text-profit' : 'text-loss'
                )}
              >
                {formatCurrency(unrealizedPnl, { showSign: true })}
              </p>
            </div>
          </div>
        </CardContent>
      </Card>

      {/* Realized P&L */}
      <Card>
        <CardContent className="p-4">
          <div className="flex items-center gap-3">
            <div
              className={cn(
                'h-10 w-10 rounded-full flex items-center justify-center',
                realizedPnl >= 0 ? 'bg-profit/10' : 'bg-loss/10'
              )}
            >
              <CheckCircle
                className={cn(
                  'h-5 w-5',
                  realizedPnl >= 0 ? 'text-profit' : 'text-loss'
                )}
              />
            </div>
            <div>
              <p className="text-sm text-muted-foreground">Realized P&L</p>
              <p
                className={cn(
                  'text-2xl font-bold tabular-nums',
                  realizedPnl >= 0 ? 'text-profit' : 'text-loss'
                )}
              >
                {formatCurrency(realizedPnl, { showSign: true })}
              </p>
            </div>
          </div>
        </CardContent>
      </Card>

      {/* Open Positions */}
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

      {/* Win Rate */}
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

      {/* Available Balance (Demo only) */}
      <Card>
        <CardContent className="p-4">
          <div className="flex items-center gap-3">
            <div className="h-10 w-10 rounded-full bg-blue-500/10 flex items-center justify-center">
              <Wallet className="h-5 w-5 text-blue-500" />
            </div>
            <div>
              <p className="text-sm text-muted-foreground">
                {isDemo ? 'Demo Cash' : 'Available'}
              </p>
              <p className="text-2xl font-bold tabular-nums">
                {formatCurrency(availableBalance ?? 0)}
              </p>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>
  );
});
