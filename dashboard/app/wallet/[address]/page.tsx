'use client';

import { useState, useMemo } from 'react';
import { useParams } from 'next/navigation';
import Link from 'next/link';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { EquityCurve } from '@/components/charts/EquityCurve';
import { useRosterStore } from '@/stores/roster-store';
import { useToastStore } from '@/stores/toast-store';
import { shortenAddress, formatCurrency } from '@/lib/utils';
import {
  ArrowLeft,
  Wallet,
  TrendingUp,
  TrendingDown,
  Shield,
  Clock,
  Target,
  AlertTriangle,
  CheckCircle,
  XCircle,
  Users,
  ChevronUp,
  ChevronDown,
  Settings,
  Zap,
  Activity,
} from 'lucide-react';
import type { TradingStyle, DecisionBrief, TradeClassification } from '@/types/api';

// Generate mock equity curve data
function generateEquityCurve(days: number, roi: number) {
  const data: { time: string; value: number }[] = [];
  let value = 100;
  const dailyReturn = Math.pow(1 + roi / 100, 1 / days) - 1;
  const now = new Date();

  for (let i = days; i >= 0; i--) {
    const date = new Date(now);
    date.setDate(date.getDate() - i);
    const randomFactor = 1 + (Math.random() - 0.5) * 0.04;
    value = value * (1 + dailyReturn) * randomFactor;
    data.push({
      time: date.toISOString().split('T')[0],
      value: Math.round(value * 100) / 100,
    });
  }
  return data;
}

// Mock wallet data
const mockWalletData = {
  address: '0x1234567890abcdef1234567890abcdef12345678',
  label: 'Alpha Trader',
  tier: 'active' as const,
  roi30d: 47.3,
  roi7d: 12.1,
  roi90d: 89.2,
  sharpe: 2.4,
  winRate: 71,
  trades: 156,
  maxDrawdown: -8.2,
  confidence: 85,
  avgTradeSize: 125,
  avgHoldTime: '4.2 hours',
  profitFactor: 2.1,
};

const mockDecisionBrief: DecisionBrief = {
  trading_style: 'event_trader',
  slippage_tolerance: 'moderate',
  preferred_categories: ['Politics', 'Sports', 'Crypto'],
  typical_hold_time: '4-8 hours',
  event_trade_ratio: 0.75,
  arb_trade_ratio: 0.25,
  fitness_score: 85,
  fitness_reasons: [
    'Strong event trading performance with 71% win rate',
    'Moderate slippage tolerance suitable for copy trading',
    'Consistent strategy over 150+ trades',
    'Drawdown within acceptable range (-8.2%)',
  ],
};

// Mock trades
const mockTrades = [
  {
    id: '1',
    market: 'Will ETH reach $5000 by March?',
    outcome: 'yes' as const,
    classification: 'event' as TradeClassification,
    entryPrice: 0.42,
    exitPrice: 0.68,
    quantity: 150,
    pnl: 39.0,
    holdTime: '6.2 hours',
    date: '2026-01-09T14:30:00Z',
  },
  {
    id: '2',
    market: 'Trump wins 2028 nomination?',
    outcome: 'yes' as const,
    classification: 'event' as TradeClassification,
    entryPrice: 0.65,
    exitPrice: 0.72,
    quantity: 200,
    pnl: 14.0,
    holdTime: '2.1 hours',
    date: '2026-01-09T10:15:00Z',
  },
  {
    id: '3',
    market: 'BTC > $100k by Feb?',
    outcome: 'no' as const,
    classification: 'arbitrage' as TradeClassification,
    entryPrice: 0.35,
    exitPrice: 0.38,
    quantity: 500,
    pnl: 15.0,
    holdTime: '3 min',
    date: '2026-01-08T22:45:00Z',
  },
];

const tradingStyleLabels: Record<TradingStyle, string> = {
  event_trader: 'Event Trader',
  arb_trader: 'Arb Trader',
  mixed: 'Mixed Strategy',
};

const tradingStyleDescriptions: Record<TradingStyle, string> = {
  event_trader: 'Focuses on directional event trades with longer hold times',
  arb_trader: 'Primarily executes mathematical arbitrage opportunities',
  mixed: 'Combines event trading and arbitrage strategies',
};

const slippageColors = {
  tight: 'text-profit',
  moderate: 'text-yellow-500',
  loose: 'text-loss',
};

export default function WalletDetailPage() {
  const params = useParams();
  const address = params.address as string;
  const toast = useToastStore();
  const { activeWallets, benchWallets, promoteToActive, demoteToBench, isRosterFull } = useRosterStore();

  // Find wallet in store or use mock
  const storedWallet = [...activeWallets, ...benchWallets].find(
    (w) => w.address.toLowerCase() === address?.toLowerCase()
  );

  const wallet = storedWallet || { ...mockWalletData, address: address || mockWalletData.address };
  const decisionBrief = (storedWallet?.decisionBrief as DecisionBrief) || mockDecisionBrief;

  const isActive = activeWallets.some((w) => w.address.toLowerCase() === address?.toLowerCase());
  const isBench = benchWallets.some((w) => w.address.toLowerCase() === address?.toLowerCase());
  const isTracked = isActive || isBench;

  const equityCurve = useMemo(() => generateEquityCurve(30, wallet.roi30d), [wallet.roi30d]);

  const handlePromote = () => {
    if (isRosterFull()) {
      toast.error('Roster Full', 'Demote a wallet first to make room');
      return;
    }
    promoteToActive(address);
    toast.success('Promoted!', `${shortenAddress(address)} added to Active 5`);
  };

  const handleDemote = () => {
    demoteToBench(address);
    toast.info('Demoted', `${shortenAddress(address)} moved to Bench`);
  };

  return (
    <div className="space-y-6">
      {/* Breadcrumb & Header */}
      <div className="flex items-center gap-4">
        <Link href={isActive ? '/roster' : '/bench'}>
          <Button variant="ghost" size="icon">
            <ArrowLeft className="h-5 w-5" />
          </Button>
        </Link>
        <div className="flex-1">
          <div className="flex items-center gap-3">
            <Wallet className="h-8 w-8" />
            <div>
              <h1 className="text-3xl font-bold tracking-tight">
                {wallet.label || shortenAddress(address)}
              </h1>
              <p className="text-muted-foreground font-mono">{shortenAddress(address)}</p>
            </div>
          </div>
        </div>
        <div className="flex items-center gap-2">
          {isActive ? (
            <span className="px-3 py-1 rounded-full bg-primary text-primary-foreground text-sm font-medium">
              Active 5
            </span>
          ) : isBench ? (
            <span className="px-3 py-1 rounded-full bg-muted text-muted-foreground text-sm font-medium">
              Bench
            </span>
          ) : (
            <span className="px-3 py-1 rounded-full bg-muted text-muted-foreground text-sm font-medium">
              Untracked
            </span>
          )}
          {isActive && (
            <Button variant="outline" onClick={handleDemote}>
              <ChevronDown className="mr-1 h-4 w-4" />
              Demote
            </Button>
          )}
          {isBench && (
            <Button onClick={handlePromote} disabled={isRosterFull()}>
              <ChevronUp className="mr-1 h-4 w-4" />
              Promote
            </Button>
          )}
          <Button variant="outline" size="icon">
            <Settings className="h-4 w-4" />
          </Button>
        </div>
      </div>

      {/* Stats Row */}
      <div className="grid gap-4 md:grid-cols-5">
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <TrendingUp className="h-5 w-5 text-profit" />
              <div>
                <p className="text-xs text-muted-foreground">ROI (30d)</p>
                <p className="text-xl font-bold text-profit">+{wallet.roi30d}%</p>
              </div>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <Target className="h-5 w-5 text-primary" />
              <div>
                <p className="text-xs text-muted-foreground">Win Rate</p>
                <p className="text-xl font-bold">{wallet.winRate}%</p>
              </div>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <Activity className="h-5 w-5 text-blue-500" />
              <div>
                <p className="text-xs text-muted-foreground">Sharpe</p>
                <p className="text-xl font-bold">{wallet.sharpe}</p>
              </div>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <TrendingDown className="h-5 w-5 text-loss" />
              <div>
                <p className="text-xs text-muted-foreground">Max Drawdown</p>
                <p className="text-xl font-bold text-loss">{wallet.maxDrawdown}%</p>
              </div>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <Zap className="h-5 w-5 text-yellow-500" />
              <div>
                <p className="text-xs text-muted-foreground">Trades</p>
                <p className="text-xl font-bold">{wallet.trades}</p>
              </div>
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Equity Curve */}
      <Card>
        <CardHeader>
          <CardTitle>Performance</CardTitle>
        </CardHeader>
        <CardContent>
          <EquityCurve data={equityCurve} height={200} />
        </CardContent>
      </Card>

      {/* Decision Brief */}
      <Card className="border-primary/50">
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Shield className="h-5 w-5" />
            Decision Brief
          </CardTitle>
          <CardDescription>
            Strategy profile and fitness assessment for copy trading
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          {/* Trading Style */}
          <div className="grid md:grid-cols-2 gap-6">
            <div className="space-y-4">
              <div>
                <p className="text-sm text-muted-foreground mb-1">Trading Style</p>
                <p className="text-lg font-semibold">
                  {tradingStyleLabels[decisionBrief.trading_style]}
                </p>
                <p className="text-sm text-muted-foreground">
                  {tradingStyleDescriptions[decisionBrief.trading_style]}
                </p>
              </div>

              <div className="grid grid-cols-2 gap-4">
                <div>
                  <p className="text-sm text-muted-foreground mb-1">Event Trades</p>
                  <p className="text-lg font-semibold">
                    {Math.round(decisionBrief.event_trade_ratio * 100)}%
                  </p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground mb-1">Arb Trades</p>
                  <p className="text-lg font-semibold">
                    {Math.round(decisionBrief.arb_trade_ratio * 100)}%
                  </p>
                </div>
              </div>

              <div>
                <p className="text-sm text-muted-foreground mb-1">Slippage Tolerance</p>
                <p className={`text-lg font-semibold capitalize ${slippageColors[decisionBrief.slippage_tolerance]}`}>
                  {decisionBrief.slippage_tolerance}
                </p>
              </div>

              <div>
                <p className="text-sm text-muted-foreground mb-1">Typical Hold Time</p>
                <p className="text-lg font-semibold">{decisionBrief.typical_hold_time}</p>
              </div>
            </div>

            <div className="space-y-4">
              <div>
                <p className="text-sm text-muted-foreground mb-1">Preferred Categories</p>
                <div className="flex flex-wrap gap-2 mt-2">
                  {decisionBrief.preferred_categories.map((cat) => (
                    <span
                      key={cat}
                      className="px-3 py-1 rounded-full bg-primary/10 text-primary text-sm"
                    >
                      {cat}
                    </span>
                  ))}
                </div>
              </div>

              <div>
                <p className="text-sm text-muted-foreground mb-2">Fitness Score</p>
                <div className="flex items-center gap-3">
                  <div className="flex-1 h-3 bg-muted rounded-full overflow-hidden">
                    <div
                      className="h-full bg-profit rounded-full transition-all"
                      style={{ width: `${decisionBrief.fitness_score}%` }}
                    />
                  </div>
                  <span className="text-lg font-bold">{decisionBrief.fitness_score}/100</span>
                </div>
              </div>
            </div>
          </div>

          {/* Fitness Reasons */}
          <div>
            <p className="text-sm text-muted-foreground mb-3 font-medium uppercase">
              Assessment
            </p>
            <ul className="space-y-2">
              {decisionBrief.fitness_reasons.map((reason, i) => (
                <li key={i} className="flex items-start gap-2 text-sm">
                  <CheckCircle className="h-4 w-4 text-profit mt-0.5 shrink-0" />
                  {reason}
                </li>
              ))}
            </ul>
          </div>
        </CardContent>
      </Card>

      {/* Trade History */}
      <Card>
        <CardHeader>
          <CardTitle>Recent Trades</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="overflow-x-auto">
            <table className="w-full">
              <thead className="border-b bg-muted/50">
                <tr>
                  <th className="text-left p-4 font-medium">Market</th>
                  <th className="text-left p-4 font-medium">Type</th>
                  <th className="text-left p-4 font-medium">Side</th>
                  <th className="text-right p-4 font-medium">Entry</th>
                  <th className="text-right p-4 font-medium">Exit</th>
                  <th className="text-right p-4 font-medium">P&L</th>
                  <th className="text-right p-4 font-medium">Hold Time</th>
                </tr>
              </thead>
              <tbody>
                {mockTrades.map((trade) => (
                  <tr key={trade.id} className="border-b hover:bg-muted/30">
                    <td className="p-4">
                      <p className="font-medium text-sm">{trade.market}</p>
                      <p className="text-xs text-muted-foreground">
                        {new Date(trade.date).toLocaleDateString()}
                      </p>
                    </td>
                    <td className="p-4">
                      <span
                        className={`px-2 py-1 rounded text-xs font-medium ${
                          trade.classification === 'event'
                            ? 'bg-blue-500/10 text-blue-500'
                            : 'bg-purple-500/10 text-purple-500'
                        }`}
                      >
                        {trade.classification === 'event' ? 'Event' : 'Arb'}
                      </span>
                    </td>
                    <td className="p-4">
                      <span
                        className={`px-2 py-1 rounded text-xs font-medium uppercase ${
                          trade.outcome === 'yes'
                            ? 'bg-profit/10 text-profit'
                            : 'bg-loss/10 text-loss'
                        }`}
                      >
                        {trade.outcome}
                      </span>
                    </td>
                    <td className="p-4 text-right tabular-nums">${trade.entryPrice.toFixed(2)}</td>
                    <td className="p-4 text-right tabular-nums">${trade.exitPrice.toFixed(2)}</td>
                    <td className="p-4 text-right">
                      <span
                        className={`tabular-nums font-medium ${
                          trade.pnl >= 0 ? 'text-profit' : 'text-loss'
                        }`}
                      >
                        {trade.pnl >= 0 ? '+' : ''}${trade.pnl.toFixed(2)}
                      </span>
                    </td>
                    <td className="p-4 text-right text-muted-foreground">{trade.holdTime}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
