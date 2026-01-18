'use client';

import { memo } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { formatCurrency, cn } from '@/lib/utils';
import { Package, TrendingUp, TrendingDown, X } from 'lucide-react';

interface Position {
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

interface ManualPositionsProps {
  positions: Position[];
  onClosePosition?: (id: string) => void;
}

export const ManualPositions = memo(function ManualPositions({
  positions,
  onClosePosition,
}: ManualPositionsProps) {
  if (positions.length === 0) {
    return null;
  }

  const totalPnl = positions.reduce((sum, p) => sum + p.pnl, 0);

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-base flex items-center gap-2">
          <Package className="h-5 w-5" />
          Manual Positions
          <span className="text-sm text-muted-foreground font-normal">
            (not from copy trading)
          </span>
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-2">
        {positions.map((position) => (
          <div
            key={position.id}
            className="flex items-center justify-between p-3 rounded-lg bg-muted/30 hover:bg-muted/50 transition-colors"
          >
            <div className="flex items-center gap-3">
              <span
                className={cn(
                  'px-2 py-0.5 rounded text-xs font-medium uppercase',
                  position.outcome === 'yes'
                    ? 'bg-profit/10 text-profit'
                    : 'bg-loss/10 text-loss'
                )}
              >
                {position.outcome}
              </span>
              <div>
                <p className="text-sm font-medium">
                  {position.marketQuestion ||
                    position.marketId.slice(0, 40) + '...'}
                </p>
                <p className="text-xs text-muted-foreground">
                  {position.quantity} @ ${position.entryPrice.toFixed(2)}
                </p>
              </div>
            </div>
            <div className="flex items-center gap-3">
              <div className="text-right">
                <div className="flex items-center justify-end gap-1">
                  {position.pnl >= 0 ? (
                    <TrendingUp className="h-3 w-3 text-profit" />
                  ) : (
                    <TrendingDown className="h-3 w-3 text-loss" />
                  )}
                  <span
                    className={cn(
                      'text-sm font-medium tabular-nums',
                      position.pnl >= 0 ? 'text-profit' : 'text-loss'
                    )}
                  >
                    {formatCurrency(position.pnl, { showSign: true })}
                  </span>
                </div>
                <p className="text-xs text-muted-foreground tabular-nums">
                  {position.pnlPercent >= 0 ? '+' : ''}
                  {position.pnlPercent.toFixed(1)}%
                </p>
              </div>
              {onClosePosition && (
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-7 w-7"
                  onClick={() => onClosePosition(position.id)}
                >
                  <X className="h-3 w-3" />
                </Button>
              )}
            </div>
          </div>
        ))}

        {/* Total P&L */}
        <div className="flex items-center justify-between p-2 border-t mt-2">
          <span className="text-sm text-muted-foreground">Total Manual P&L</span>
          <span
            className={cn(
              'text-sm font-medium',
              totalPnl >= 0 ? 'text-profit' : 'text-loss'
            )}
          >
            {formatCurrency(totalPnl, { showSign: true })}
          </span>
        </div>
      </CardContent>
    </Card>
  );
});
