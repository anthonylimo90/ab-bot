'use client';

import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { Wallet, ArrowRight, Bot } from 'lucide-react';
import Link from 'next/link';
import { shortenAddress } from '@/lib/utils';
import type { WorkspaceAllocation } from '@/types/api';

interface ActiveWalletsCardProps {
  wallets: WorkspaceAllocation[] | undefined;
  isLoading: boolean;
}

export function ActiveWalletsCard({ wallets, isLoading }: ActiveWalletsCardProps) {
  if (isLoading) {
    return (
      <Card>
        <CardHeader>
          <Skeleton className="h-6 w-32" />
        </CardHeader>
        <CardContent className="space-y-3">
          {Array.from({ length: 5 }).map((_, i) => (
            <Skeleton key={i} className="h-12 w-full" />
          ))}
        </CardContent>
      </Card>
    );
  }

  const activeWallets = wallets ?? [];
  const emptySlots = 5 - activeWallets.length;

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between">
        <CardTitle className="flex items-center gap-2">
          <Wallet className="h-5 w-5" />
          Active Wallets
        </CardTitle>
        <Badge variant="outline">{activeWallets.length}/5</Badge>
      </CardHeader>
      <CardContent className="space-y-3">
        {activeWallets.map((wallet) => (
          <div
            key={wallet.wallet_address}
            className="flex items-center justify-between p-3 rounded-lg border"
          >
            <div className="flex items-center gap-3">
              <div className="flex h-8 w-8 items-center justify-center rounded-full bg-primary/10">
                <Wallet className="h-4 w-4 text-primary" />
              </div>
              <div>
                <div className="flex items-center gap-2">
                  <span className="font-mono text-sm font-medium">
                    {shortenAddress(wallet.wallet_address)}
                  </span>
                  {wallet.auto_assigned && (
                    <Badge variant="secondary" className="text-xs">
                      <Bot className="h-3 w-3 mr-1" />
                      Auto
                    </Badge>
                  )}
                </div>
                <p className="text-xs text-muted-foreground">
                  {wallet.allocation_pct}% allocation
                </p>
              </div>
            </div>
            <div className="text-right text-sm">
              <div className="grid grid-cols-3 gap-3">
                <div>
                  <p className="text-xs text-muted-foreground">ROI</p>
                  <p className="font-medium text-profit">
                    +{(wallet.backtest_roi ?? 0).toFixed(1)}%
                  </p>
                </div>
                <div>
                  <p className="text-xs text-muted-foreground">Sharpe</p>
                  <p className="font-medium">
                    {(wallet.backtest_sharpe ?? 0).toFixed(2)}
                  </p>
                </div>
                <div>
                  <p className="text-xs text-muted-foreground">Win</p>
                  <p className="font-medium">
                    {(wallet.backtest_win_rate ?? 0).toFixed(0)}%
                  </p>
                </div>
              </div>
            </div>
          </div>
        ))}

        {/* Empty Slots */}
        {Array.from({ length: emptySlots }).map((_, i) => (
          <div
            key={`empty-${i}`}
            className="flex items-center justify-center p-3 rounded-lg border border-dashed text-muted-foreground"
          >
            <span className="text-sm">Empty slot</span>
          </div>
        ))}

        {/* View Full Roster Link */}
        <Link href="/allocate">
          <Button variant="outline" className="w-full mt-2">
            View Full Roster
            <ArrowRight className="ml-2 h-4 w-4" />
          </Button>
        </Link>
      </CardContent>
    </Card>
  );
}
