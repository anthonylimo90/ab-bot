'use client';

import { useState } from 'react';
import Link from 'next/link';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { useRosterStore, RosterWallet } from '@/stores/roster-store';
import { useToastStore } from '@/stores/toast-store';
import { shortenAddress, formatCurrency } from '@/lib/utils';
import {
  Users,
  Plus,
  TrendingUp,
  TrendingDown,
  ArrowRight,
  Settings,
  ChevronDown,
  AlertTriangle,
} from 'lucide-react';

// Mock data for demo - in real app this would come from store
const mockActiveWallets: RosterWallet[] = [
  {
    address: '0x1234567890abcdef1234567890abcdef12345678',
    label: 'Alpha Trader',
    tier: 'active',
    copySettings: {
      copy_behavior: 'events_only',
      allocation_pct: 25,
      max_position_size: 200,
    },
    roi30d: 47.3,
    sharpe: 2.4,
    winRate: 71,
    trades: 156,
    maxDrawdown: -8.2,
    confidence: 85,
    addedAt: '2026-01-01T00:00:00Z',
    lastActivity: '2026-01-09T14:30:00Z',
  },
  {
    address: '0xabcdef1234567890abcdef1234567890abcdef12',
    label: 'Event Specialist',
    tier: 'active',
    copySettings: {
      copy_behavior: 'copy_all',
      allocation_pct: 20,
      max_position_size: 150,
    },
    roi30d: 38.1,
    sharpe: 1.9,
    winRate: 68,
    trades: 89,
    maxDrawdown: -12.1,
    confidence: 72,
    addedAt: '2026-01-03T00:00:00Z',
    lastActivity: '2026-01-09T12:15:00Z',
  },
];

const copyBehaviorLabels = {
  copy_all: 'All Trades',
  events_only: 'Events Only',
  arb_threshold: 'Arb Threshold',
};

function EmptySlotCard({ slotNumber }: { slotNumber: number }) {
  return (
    <Card className="border-dashed">
      <CardContent className="p-6 flex flex-col items-center justify-center min-h-[200px] text-center">
        <div className="h-12 w-12 rounded-full bg-muted flex items-center justify-center mb-4">
          <Plus className="h-6 w-6 text-muted-foreground" />
        </div>
        <p className="font-medium text-muted-foreground mb-2">Slot {slotNumber} Available</p>
        <p className="text-sm text-muted-foreground mb-4">
          Add a wallet from the Bench to start copying
        </p>
        <Link href="/bench">
          <Button variant="outline" size="sm">
            Browse Bench
            <ArrowRight className="ml-2 h-4 w-4" />
          </Button>
        </Link>
      </CardContent>
    </Card>
  );
}

function RosterCard({ wallet }: { wallet: RosterWallet }) {
  const toast = useToastStore();
  const { demoteToBench } = useRosterStore();

  const handleDemote = () => {
    demoteToBench(wallet.address);
    toast.info(
      'Moved to Bench',
      `${wallet.label || shortenAddress(wallet.address)} has been demoted to the Bench`
    );
  };

  return (
    <Card className="hover:border-primary transition-colors">
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="h-10 w-10 rounded-full bg-primary flex items-center justify-center text-primary-foreground font-bold">
              {wallet.label?.charAt(0) || wallet.address.charAt(2).toUpperCase()}
            </div>
            <div>
              <CardTitle className="text-base">
                {wallet.label || shortenAddress(wallet.address)}
              </CardTitle>
              <p className="text-xs text-muted-foreground font-mono">
                {shortenAddress(wallet.address)}
              </p>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <span className="text-xs px-2 py-1 rounded-full bg-primary/10 text-primary">
              {copyBehaviorLabels[wallet.copySettings.copy_behavior]}
            </span>
            <Button variant="ghost" size="icon" className="h-8 w-8">
              <Settings className="h-4 w-4" />
            </Button>
          </div>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {/* Metrics Row */}
        <div className="grid grid-cols-4 gap-4 text-sm">
          <div>
            <p className="text-xs text-muted-foreground">ROI (30d)</p>
            <p className={`font-medium ${wallet.roi30d >= 0 ? 'text-profit' : 'text-loss'}`}>
              {wallet.roi30d >= 0 ? '+' : ''}{wallet.roi30d}%
            </p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Sharpe</p>
            <p className="font-medium">{wallet.sharpe}</p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Win Rate</p>
            <p className="font-medium">{wallet.winRate}%</p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Max DD</p>
            <p className="font-medium text-loss">{wallet.maxDrawdown}%</p>
          </div>
        </div>

        {/* Allocation Bar */}
        <div className="space-y-1">
          <div className="flex items-center justify-between text-xs">
            <span className="text-muted-foreground">Allocation</span>
            <span className="font-medium">{wallet.copySettings.allocation_pct}%</span>
          </div>
          <div className="w-full bg-muted rounded-full h-2">
            <div
              className="bg-primary h-2 rounded-full transition-all"
              style={{ width: `${wallet.copySettings.allocation_pct}%` }}
            />
          </div>
        </div>

        {/* Actions */}
        <div className="flex items-center justify-between pt-2">
          <Link href={`/wallet/${wallet.address}`}>
            <Button variant="outline" size="sm">
              View Details
              <ArrowRight className="ml-2 h-4 w-4" />
            </Button>
          </Link>
          <Button variant="ghost" size="sm" onClick={handleDemote}>
            <ChevronDown className="mr-1 h-4 w-4" />
            Demote
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

export default function RosterPage() {
  const { activeWallets } = useRosterStore();
  const toast = useToastStore();

  // Use mock data if store is empty (demo mode)
  const displayWallets = activeWallets.length > 0 ? activeWallets : mockActiveWallets;
  const emptySlots = 5 - displayWallets.length;

  const totalAllocation = displayWallets.reduce(
    (sum, w) => sum + w.copySettings.allocation_pct,
    0
  );

  return (
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight flex items-center gap-2">
            <Users className="h-8 w-8" />
            Active 5
          </h1>
          <p className="text-muted-foreground">
            Your top-tier wallets being actively copied
          </p>
        </div>
        <Link href="/bench">
          <Button>
            <Plus className="mr-2 h-4 w-4" />
            Add from Bench
          </Button>
        </Link>
      </div>

      {/* Summary Stats */}
      <div className="grid gap-4 md:grid-cols-4">
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <div className="h-10 w-10 rounded-full bg-primary/10 flex items-center justify-center">
                <Users className="h-5 w-5 text-primary" />
              </div>
              <div>
                <p className="text-sm text-muted-foreground">Active Wallets</p>
                <p className="text-2xl font-bold">{displayWallets.length}/5</p>
              </div>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <div className="h-10 w-10 rounded-full bg-profit/10 flex items-center justify-center">
                <TrendingUp className="h-5 w-5 text-profit" />
              </div>
              <div>
                <p className="text-sm text-muted-foreground">Avg. ROI (30d)</p>
                <p className="text-2xl font-bold text-profit">
                  +{(displayWallets.reduce((sum, w) => sum + w.roi30d, 0) / displayWallets.length || 0).toFixed(1)}%
                </p>
              </div>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <div className="h-10 w-10 rounded-full bg-demo/10 flex items-center justify-center">
                <TrendingDown className="h-5 w-5 text-demo" />
              </div>
              <div>
                <p className="text-sm text-muted-foreground">Total Allocation</p>
                <p className="text-2xl font-bold">{totalAllocation}%</p>
              </div>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <div className="h-10 w-10 rounded-full bg-yellow-500/10 flex items-center justify-center">
                <AlertTriangle className="h-5 w-5 text-yellow-500" />
              </div>
              <div>
                <p className="text-sm text-muted-foreground">Avg. Max DD</p>
                <p className="text-2xl font-bold text-loss">
                  {(displayWallets.reduce((sum, w) => sum + w.maxDrawdown, 0) / displayWallets.length || 0).toFixed(1)}%
                </p>
              </div>
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Roster Grid */}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {displayWallets.map((wallet) => (
          <RosterCard key={wallet.address} wallet={wallet} />
        ))}
        {Array.from({ length: emptySlots }).map((_, i) => (
          <EmptySlotCard key={`empty-${i}`} slotNumber={displayWallets.length + i + 1} />
        ))}
      </div>
    </div>
  );
}
