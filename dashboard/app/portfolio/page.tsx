'use client';

import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { MetricCard } from '@/components/shared/MetricCard';
import { ConnectionStatus } from '@/components/shared/ConnectionStatus';
import { LiveIndicator } from '@/components/shared/LiveIndicator';
import { usePositions } from '@/hooks/usePositions';
import { formatCurrency, shortenAddress } from '@/lib/utils';
import { Download, Filter, X, TrendingUp, TrendingDown, RefreshCw } from 'lucide-react';
import { cn } from '@/lib/utils';

export default function PortfolioPage() {
  const {
    openPositions,
    closedPositions,
    status,
    totalUnrealizedPnl,
    refresh,
    closePosition,
  } = usePositions();

  const totalValue = openPositions.reduce(
    (sum, p) => sum + p.quantity * p.current_price,
    0
  );
  const totalFees = -45.67; // Mock for now
  const totalRealizedPnl = 0; // Would come from closed positions API

  const sourceBreakdown = [
    {
      source: 'Copy Trades',
      amount: openPositions
        .filter((p) => p.is_copy_trade)
        .reduce((sum, p) => sum + p.quantity * p.current_price, 0),
    },
    {
      source: 'Manual',
      amount: openPositions
        .filter((p) => !p.is_copy_trade)
        .reduce((sum, p) => sum + p.quantity * p.current_price, 0),
    },
  ].map((item) => ({
    ...item,
    percent: totalValue > 0 ? Math.round((item.amount / totalValue) * 100) : 0,
  }));

  return (
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-4">
          <div>
            <h1 className="text-3xl font-bold tracking-tight">Portfolio</h1>
            <p className="text-muted-foreground">
              Manage your positions and track performance
            </p>
          </div>
          <LiveIndicator />
          <ConnectionStatus status={status} showLabel />
        </div>
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={refresh}>
            <RefreshCw className="mr-2 h-4 w-4" />
            Refresh
          </Button>
          <Button variant="outline">
            <Filter className="mr-2 h-4 w-4" />
            Filters
          </Button>
          <Button variant="outline">
            <Download className="mr-2 h-4 w-4" />
            Export
          </Button>
        </div>
      </div>

      {/* Stats */}
      <div className="grid gap-4 md:grid-cols-4">
        <MetricCard
          title="Total Value"
          value={formatCurrency(totalValue)}
          trend="neutral"
        />
        <MetricCard
          title="Unrealized P&L"
          value={formatCurrency(totalUnrealizedPnl, { showSign: true })}
          trend={totalUnrealizedPnl >= 0 ? 'up' : 'down'}
        />
        <MetricCard
          title="Realized P&L"
          value={formatCurrency(totalRealizedPnl, { showSign: true })}
          trend={totalRealizedPnl >= 0 ? 'up' : 'down'}
        />
        <MetricCard
          title="Total Fees"
          value={formatCurrency(totalFees)}
          trend="down"
        />
      </div>

      {/* Positions */}
      <Tabs defaultValue="open" className="space-y-4">
        <TabsList>
          <TabsTrigger value="open">
            Open Positions ({openPositions.length})
          </TabsTrigger>
          <TabsTrigger value="closed">
            Closed ({closedPositions.length})
          </TabsTrigger>
          <TabsTrigger value="history">History</TabsTrigger>
        </TabsList>

        <TabsContent value="open">
          <Card>
            <CardContent className="p-0">
              <div className="overflow-x-auto">
                <table className="w-full">
                  <thead className="border-b bg-muted/50">
                    <tr>
                      <th className="text-left p-4 font-medium">Market</th>
                      <th className="text-left p-4 font-medium">Side</th>
                      <th className="text-right p-4 font-medium">Qty</th>
                      <th className="text-right p-4 font-medium">Entry</th>
                      <th className="text-right p-4 font-medium">
                        <div className="flex items-center justify-end gap-1">
                          Current
                          <LiveIndicator label="" />
                        </div>
                      </th>
                      <th className="text-right p-4 font-medium">P&L</th>
                      <th className="text-right p-4 font-medium">Actions</th>
                    </tr>
                  </thead>
                  <tbody>
                    {openPositions.map((position) => (
                      <tr
                        key={position.id}
                        className="border-b hover:bg-muted/30 transition-colors"
                      >
                        <td className="p-4">
                          <div>
                            <p className="font-medium">{position.market_id}</p>
                            <p className="text-xs text-muted-foreground">
                              {position.is_copy_trade && position.source_wallet
                                ? `Copy: ${shortenAddress(position.source_wallet)}`
                                : 'Manual'}
                            </p>
                            {position.stop_loss && (
                              <p className="text-xs text-muted-foreground">
                                Stop: ${position.stop_loss}
                              </p>
                            )}
                          </div>
                        </td>
                        <td className="p-4">
                          <span
                            className={cn(
                              'px-2 py-1 rounded text-xs font-medium uppercase',
                              position.outcome === 'yes'
                                ? 'bg-profit/10 text-profit'
                                : 'bg-loss/10 text-loss'
                            )}
                          >
                            {position.outcome}
                          </span>
                        </td>
                        <td className="p-4 text-right tabular-nums">
                          {position.quantity}
                        </td>
                        <td className="p-4 text-right tabular-nums">
                          ${position.entry_price.toFixed(2)}
                        </td>
                        <td className="p-4 text-right">
                          <span
                            className={cn(
                              'tabular-nums font-medium transition-colors',
                              position.current_price > position.entry_price
                                ? 'text-profit'
                                : position.current_price < position.entry_price
                                ? 'text-loss'
                                : ''
                            )}
                          >
                            ${position.current_price.toFixed(2)}
                          </span>
                        </td>
                        <td className="p-4 text-right">
                          <div className="flex items-center justify-end gap-1">
                            {position.unrealized_pnl >= 0 ? (
                              <TrendingUp className="h-4 w-4 text-profit" />
                            ) : (
                              <TrendingDown className="h-4 w-4 text-loss" />
                            )}
                            <span
                              className={cn(
                                'tabular-nums font-medium',
                                position.unrealized_pnl >= 0 ? 'text-profit' : 'text-loss'
                              )}
                            >
                              {position.unrealized_pnl >= 0 ? '+' : ''}
                              {formatCurrency(position.unrealized_pnl)}
                            </span>
                          </div>
                        </td>
                        <td className="p-4 text-right">
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => closePosition(position.id)}
                          >
                            <X className="h-4 w-4" />
                            Close
                          </Button>
                        </td>
                      </tr>
                    ))}
                    {openPositions.length === 0 && (
                      <tr>
                        <td colSpan={7} className="p-8 text-center text-muted-foreground">
                          No open positions
                        </td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="closed">
          <Card>
            <CardContent className="py-10">
              <p className="text-center text-muted-foreground">
                {closedPositions.length === 0
                  ? 'No closed positions yet'
                  : `${closedPositions.length} closed positions`}
              </p>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="history">
          <Card>
            <CardContent className="py-10">
              <p className="text-center text-muted-foreground">
                Transaction history will appear here
              </p>
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>

      {/* Source Breakdown */}
      <Card>
        <CardHeader>
          <CardTitle>Position by Source</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="space-y-4">
            {sourceBreakdown.map((item) => (
              <div key={item.source} className="space-y-2">
                <div className="flex items-center justify-between text-sm">
                  <span>{item.source}</span>
                  <span className="font-medium">
                    {formatCurrency(item.amount)} ({item.percent}%)
                  </span>
                </div>
                <div className="h-2 rounded-full bg-muted overflow-hidden">
                  <div
                    className="h-full bg-primary transition-all duration-500"
                    style={{ width: `${item.percent}%` }}
                  />
                </div>
              </div>
            ))}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
