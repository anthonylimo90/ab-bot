'use client';

import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { LiveIndicator } from '@/components/shared/LiveIndicator';
import { cn, formatCurrency, shortenAddress } from '@/lib/utils';
import { TimeAgo } from '@/components/shared/TimeAgo';
import type { LiveTrade } from '@/types/api';
import { useLiveTradesQuery } from '@/hooks/queries/useDiscoverQuery';

interface LiveActivityFeedProps {
  className?: string;
  maxItems?: number;
}

export function LiveActivityFeed({
  className,
  maxItems = 10,
}: LiveActivityFeedProps) {
  const { data: trades = [], isLoading, error } = useLiveTradesQuery({
    limit: maxItems,
  });

  return (
    <Card className={cn('overflow-hidden', className)}>
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <CardTitle className="text-lg">Live Activity</CardTitle>
          <LiveIndicator />
        </div>
      </CardHeader>
      <CardContent className="p-0">
        {isLoading ? (
          <div className="flex items-center justify-center py-8">
            <div className="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent" />
          </div>
        ) : error ? (
          <div className="px-6 py-4 text-center text-sm text-muted-foreground">
            {error instanceof Error ? error.message : 'Failed to fetch trades'}
          </div>
        ) : trades.length === 0 ? (
          <div className="px-6 py-4 text-center text-sm text-muted-foreground">
            No recent trades
          </div>
        ) : (
          <div className="divide-y divide-border">
            {trades.map((trade) => (
              <TradeRow key={trade.tx_hash} trade={trade} />
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function TradeRow({ trade }: { trade: LiveTrade }) {
  const isBuy = trade.direction === 'buy';

  return (
    <div className="flex items-center gap-3 px-4 py-3 hover:bg-muted/50 transition-colors">
      {/* Direction indicator */}
      <div
        className={cn(
          'flex h-8 w-8 shrink-0 items-center justify-center rounded-full text-xs font-bold',
          isBuy
            ? 'bg-profit/10 text-profit'
            : 'bg-loss/10 text-loss'
        )}
      >
        {isBuy ? 'B' : 'S'}
      </div>

      {/* Trade details */}
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="font-medium truncate">
            {trade.wallet_label || shortenAddress(trade.wallet_address)}
          </span>
          <TimeAgo
            date={trade.timestamp}
            className="text-muted-foreground text-xs"
          />
        </div>
        <div className="text-sm text-muted-foreground truncate">
          {trade.market_question || shortenAddress(trade.market_id)}
        </div>
      </div>

      {/* Trade value */}
      <div className="text-right shrink-0">
        <div className={cn('font-medium', isBuy ? 'text-profit' : 'text-loss')}>
          {isBuy ? '+' : '-'}{formatCurrency(trade.value)}
        </div>
        <div className="text-xs text-muted-foreground">
          {trade.outcome} @ {(trade.price * 100).toFixed(0)}¢
        </div>
      </div>
    </div>
  );
}

export default LiveActivityFeed;
