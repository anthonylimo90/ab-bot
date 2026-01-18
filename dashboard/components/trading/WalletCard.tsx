'use client';

import { useState, memo } from 'react';
import Link from 'next/link';
import { Card, CardContent, CardHeader } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Slider } from '@/components/ui/slider';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { shortenAddress, formatCurrency, cn } from '@/lib/utils';
import {
  ChevronDown,
  ChevronUp,
  ArrowRight,
  Settings,
  TrendingUp,
  TrendingDown,
  X,
  Trash2,
} from 'lucide-react';
import { useRosterStore, type RosterWallet } from '@/stores/roster-store';
import type { DemoPosition } from '@/stores/demo-portfolio-store';
import type { CopyBehavior } from '@/types/api';

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

interface WalletCardProps {
  wallet: RosterWallet;
  positions: Position[];
  onDemote?: (address: string) => void;
  onPromote?: (address: string) => void;
  onRemove?: (address: string) => void;
  onClosePosition?: (id: string) => void;
  isActive?: boolean;
  isRosterFull?: boolean;
  isLoading?: boolean;
}

export const WalletCard = memo(function WalletCard({
  wallet,
  positions,
  onDemote,
  onPromote,
  onRemove,
  onClosePosition,
  isActive = false,
  isRosterFull = false,
  isLoading = false,
}: WalletCardProps) {
  const [isExpanded, setIsExpanded] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const { updateCopySettings } = useRosterStore();

  const totalPnl = positions.reduce((sum, p) => sum + p.pnl, 0);
  const hasPositions = positions.length > 0;

  const copyBehaviorLabels: Record<string, string> = {
    copy_all: 'All Trades',
    events_only: 'Events Only',
    arb_threshold: 'Arb Threshold',
  };

  const handleAllocationChange = (value: number[]) => {
    updateCopySettings(wallet.address, { allocation_pct: value[0] });
  };

  const handleBehaviorChange = (value: CopyBehavior) => {
    updateCopySettings(wallet.address, { copy_behavior: value });
  };

  const handleMaxPositionChange = (value: number[]) => {
    updateCopySettings(wallet.address, { max_position_size: value[0] });
  };

  return (
    <Card className="hover:border-primary/50 transition-colors">
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div
              className={cn(
                'h-10 w-10 rounded-full flex items-center justify-center font-bold text-primary-foreground',
                isActive ? 'bg-primary' : 'bg-muted text-muted-foreground'
              )}
            >
              {wallet.label?.charAt(0) || wallet.address.charAt(2).toUpperCase()}
            </div>
            <div>
              <p className="font-medium">
                {wallet.label || shortenAddress(wallet.address)}
              </p>
              <p className="text-xs text-muted-foreground font-mono">
                {shortenAddress(wallet.address)}
              </p>
            </div>
          </div>
          <div className="flex items-center gap-2">
            {isActive && wallet.copySettings && (
              <span className="text-xs px-2 py-1 rounded-full bg-primary/10 text-primary">
                {copyBehaviorLabels[wallet.copySettings.copy_behavior] || 'All Trades'}
              </span>
            )}
            {isActive && (
              <Button
                variant={showSettings ? 'secondary' : 'ghost'}
                size="icon"
                className="h-8 w-8"
                onClick={() => setShowSettings(!showSettings)}
              >
                <Settings className="h-4 w-4" />
              </Button>
            )}
          </div>
        </div>
      </CardHeader>

      <CardContent className="space-y-4">
        {/* Metrics Row */}
        <div className="grid grid-cols-4 gap-4 text-sm">
          <div>
            <p className="text-xs text-muted-foreground">ROI (30d)</p>
            <p
              className={cn(
                'font-medium',
                wallet.roi30d >= 0 ? 'text-profit' : 'text-loss'
              )}
            >
              {wallet.roi30d >= 0 ? '+' : ''}
              {Number(wallet.roi30d).toFixed(1)}%
            </p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Sharpe</p>
            <p className="font-medium">{Number(wallet.sharpe).toFixed(2)}</p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">Win Rate</p>
            <p className="font-medium">{Number(wallet.winRate).toFixed(1)}%</p>
          </div>
          <div>
            <p className="text-xs text-muted-foreground">
              {isActive ? 'Allocation' : 'Max DD'}
            </p>
            <p className={cn('font-medium', !isActive && 'text-loss')}>
              {isActive
                ? `${wallet.copySettings?.allocation_pct || 0}%`
                : `${Number(wallet.maxDrawdown).toFixed(1)}%`}
            </p>
          </div>
        </div>

        {/* Allocation Bar (Active only) */}
        {isActive && wallet.copySettings && (
          <div className="space-y-1">
            <div className="w-full bg-muted rounded-full h-2">
              <div
                className="bg-primary h-2 rounded-full transition-all"
                style={{ width: `${wallet.copySettings.allocation_pct}%` }}
              />
            </div>
          </div>
        )}

        {/* Settings Panel (Active wallets only) */}
        {isActive && showSettings && wallet.copySettings && (
          <div className="p-4 bg-muted/30 rounded-lg space-y-4 border">
            <p className="text-xs font-medium text-muted-foreground uppercase tracking-wider">
              Copy Settings
            </p>

            {/* Allocation Slider */}
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <label className="text-sm font-medium">Allocation</label>
                <span className="text-sm tabular-nums">
                  {wallet.copySettings.allocation_pct}%
                </span>
              </div>
              <Slider
                value={[wallet.copySettings.allocation_pct]}
                onValueChange={handleAllocationChange}
                min={0}
                max={100}
                step={5}
                className="w-full"
              />
            </div>

            {/* Copy Behavior */}
            <div className="space-y-2">
              <label className="text-sm font-medium">Copy Behavior</label>
              <Select
                value={wallet.copySettings.copy_behavior}
                onValueChange={(v) => handleBehaviorChange(v as CopyBehavior)}
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="Select behavior" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="copy_all">All Trades</SelectItem>
                  <SelectItem value="events_only">Events Only</SelectItem>
                  <SelectItem value="arb_threshold">Arb Threshold</SelectItem>
                </SelectContent>
              </Select>
            </div>

            {/* Max Position Size */}
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <label className="text-sm font-medium">Max Position Size</label>
                <span className="text-sm tabular-nums">
                  ${wallet.copySettings.max_position_size}
                </span>
              </div>
              <Slider
                value={[wallet.copySettings.max_position_size]}
                onValueChange={handleMaxPositionChange}
                min={10}
                max={500}
                step={10}
                className="w-full"
              />
            </div>
          </div>
        )}

        {/* Actions Row */}
        <div className="flex items-center justify-between pt-2 border-t">
          <div className="flex items-center gap-2">
            <Link href={`/wallet/${wallet.address}`}>
              <Button variant="outline" size="sm">
                View Details
                <ArrowRight className="ml-2 h-4 w-4" />
              </Button>
            </Link>
            {isActive ? (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => onDemote?.(wallet.address)}
                disabled={isLoading}
              >
                <ChevronDown className="mr-1 h-4 w-4" />
                Demote
              </Button>
            ) : (
              <>
                <Button
                  variant="default"
                  size="sm"
                  onClick={() => onPromote?.(wallet.address)}
                  disabled={isLoading || isRosterFull}
                >
                  <ChevronUp className="mr-1 h-4 w-4" />
                  Promote
                </Button>
                {onRemove && (
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={() => onRemove(wallet.address)}
                    disabled={isLoading}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                )}
              </>
            )}
          </div>

          {/* Expand/Collapse for positions */}
          {isActive && hasPositions && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setIsExpanded(!isExpanded)}
            >
              {isExpanded ? (
                <>
                  <ChevronUp className="mr-1 h-4 w-4" />
                  Hide Positions
                </>
              ) : (
                <>
                  <ChevronDown className="mr-1 h-4 w-4" />
                  {positions.length} Position{positions.length !== 1 ? 's' : ''}
                </>
              )}
            </Button>
          )}
        </div>

        {/* Positions (Expanded) */}
        {isActive && isExpanded && hasPositions && (
          <div className="space-y-2 pt-2 border-t">
            <p className="text-xs font-medium text-muted-foreground uppercase tracking-wider">
              Positions from this wallet
            </p>
            <div className="space-y-2">
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
                          position.marketId.slice(0, 30) + '...'}
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

              {/* Total P&L for this wallet */}
              <div className="flex items-center justify-between p-2 border-t">
                <span className="text-sm text-muted-foreground">
                  Wallet P&L
                </span>
                <span
                  className={cn(
                    'text-sm font-medium',
                    totalPnl >= 0 ? 'text-profit' : 'text-loss'
                  )}
                >
                  {formatCurrency(totalPnl, { showSign: true })}
                </span>
              </div>
            </div>
          </div>
        )}

        {/* No Positions Message */}
        {isActive && isExpanded && !hasPositions && (
          <div className="pt-2 border-t text-center py-4">
            <p className="text-sm text-muted-foreground">
              No positions from this wallet yet
            </p>
          </div>
        )}
      </CardContent>
    </Card>
  );
});
