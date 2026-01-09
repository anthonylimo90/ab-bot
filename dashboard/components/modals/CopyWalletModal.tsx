'use client';

import { useState } from 'react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Slider } from '@/components/ui/slider';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Wallet, Users, AlertTriangle, Zap, Calendar } from 'lucide-react';
import { shortenAddress, formatCurrency } from '@/lib/utils';
import type { CopyBehavior } from '@/types/api';

interface WalletInfo {
  address: string;
  roi30d?: number;
  sharpe?: number;
  winRate?: number;
  trades?: number;
  confidence?: number;
}

interface CopyWalletModalProps {
  wallet: WalletInfo | null;
  isOpen: boolean;
  onClose: () => void;
  onConfirm: (settings: {
    address: string;
    allocation_pct: number;
    copy_behavior: CopyBehavior;
    max_position_size: number;
    tier: 'active' | 'bench';
  }) => void;
  rosterCount: number;
  maxRoster?: number;
}

const copyBehaviorLabels: Record<CopyBehavior, { label: string; description: string }> = {
  copy_all: {
    label: 'Copy All Trades',
    description: 'Mirror all trades from this wallet',
  },
  events_only: {
    label: 'Events Only',
    description: 'Only copy directional event trades, skip arbitrage',
  },
  arb_threshold: {
    label: 'Arb Threshold',
    description: 'Replicate arb logic only when spread exceeds threshold',
  },
};

export function CopyWalletModal({
  wallet,
  isOpen,
  onClose,
  onConfirm,
  rosterCount,
  maxRoster = 5,
}: CopyWalletModalProps) {
  const [allocation, setAllocation] = useState(10);
  const [copyBehavior, setCopyBehavior] = useState<CopyBehavior>('events_only');
  const [maxPosition, setMaxPosition] = useState(100);
  const [tier, setTier] = useState<'active' | 'bench'>('bench');

  const canAddToActive = rosterCount < maxRoster;
  const slotsRemaining = maxRoster - rosterCount;

  const handleConfirm = () => {
    if (!wallet) return;
    onConfirm({
      address: wallet.address,
      allocation_pct: allocation,
      copy_behavior: copyBehavior,
      max_position_size: maxPosition,
      tier,
    });
    onClose();
  };

  if (!wallet) return null;

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-[500px]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Wallet className="h-5 w-5" />
            Copy Wallet
          </DialogTitle>
          <DialogDescription>
            Configure copy trading settings for this wallet
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-6 py-4">
          {/* Wallet Info */}
          <div className="rounded-lg border p-4 space-y-3">
            <div className="flex items-center justify-between">
              <span className="font-mono font-medium">
                {shortenAddress(wallet.address)}
              </span>
              {wallet.confidence && (
                <span className="text-xs px-2 py-1 rounded-full bg-demo/10 text-demo">
                  {wallet.confidence}% confidence
                </span>
              )}
            </div>
            <div className="grid grid-cols-4 gap-2 text-sm">
              {wallet.roi30d !== undefined && (
                <div>
                  <p className="text-xs text-muted-foreground">ROI</p>
                  <p className="font-medium text-profit">+{wallet.roi30d}%</p>
                </div>
              )}
              {wallet.sharpe !== undefined && (
                <div>
                  <p className="text-xs text-muted-foreground">Sharpe</p>
                  <p className="font-medium">{wallet.sharpe}</p>
                </div>
              )}
              {wallet.winRate !== undefined && (
                <div>
                  <p className="text-xs text-muted-foreground">Win Rate</p>
                  <p className="font-medium">{wallet.winRate}%</p>
                </div>
              )}
              {wallet.trades !== undefined && (
                <div>
                  <p className="text-xs text-muted-foreground">Trades</p>
                  <p className="font-medium">{wallet.trades}</p>
                </div>
              )}
            </div>
          </div>

          {/* Tier Selection */}
          <div className="space-y-3">
            <label className="text-sm font-medium flex items-center gap-2">
              <Users className="h-4 w-4" />
              Add to
            </label>
            <div className="grid grid-cols-2 gap-3">
              <button
                type="button"
                onClick={() => setTier('active')}
                disabled={!canAddToActive}
                className={`p-3 rounded-lg border text-left transition-colors ${
                  tier === 'active'
                    ? 'border-primary bg-primary/5'
                    : 'border-border hover:border-muted-foreground'
                } ${!canAddToActive ? 'opacity-50 cursor-not-allowed' : ''}`}
              >
                <div className="font-medium">Active 5</div>
                <div className="text-xs text-muted-foreground">
                  {canAddToActive
                    ? `${slotsRemaining} slot${slotsRemaining !== 1 ? 's' : ''} available`
                    : 'Roster full'}
                </div>
              </button>
              <button
                type="button"
                onClick={() => setTier('bench')}
                className={`p-3 rounded-lg border text-left transition-colors ${
                  tier === 'bench'
                    ? 'border-primary bg-primary/5'
                    : 'border-border hover:border-muted-foreground'
                }`}
              >
                <div className="font-medium">Bench</div>
                <div className="text-xs text-muted-foreground">
                  Monitor & evaluate
                </div>
              </button>
            </div>
            {!canAddToActive && tier === 'bench' && (
              <p className="text-xs text-muted-foreground flex items-center gap-1">
                <AlertTriangle className="h-3 w-3" />
                Active roster is full. Demote a wallet first to add to Active 5.
              </p>
            )}
          </div>

          {/* Copy Behavior */}
          <div className="space-y-3">
            <label className="text-sm font-medium flex items-center gap-2">
              <Zap className="h-4 w-4" />
              Copy Behavior
            </label>
            <Select value={copyBehavior} onValueChange={(v) => setCopyBehavior(v as CopyBehavior)}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {Object.entries(copyBehaviorLabels).map(([key, { label, description }]) => (
                  <SelectItem key={key} value={key}>
                    <div>
                      <div className="font-medium">{label}</div>
                      <div className="text-xs text-muted-foreground">{description}</div>
                    </div>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          {/* Allocation */}
          {tier === 'active' && (
            <div className="space-y-3">
              <div className="flex items-center justify-between">
                <label className="text-sm font-medium">Allocation</label>
                <span className="text-sm font-medium">{allocation}%</span>
              </div>
              <Slider
                value={[allocation]}
                onValueChange={([v]) => setAllocation(v)}
                min={5}
                max={50}
                step={5}
              />
              <p className="text-xs text-muted-foreground">
                Percentage of your capital allocated to copying this wallet
              </p>
            </div>
          )}

          {/* Max Position */}
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <label className="text-sm font-medium">Max Position Size</label>
              <span className="text-sm font-medium">{formatCurrency(maxPosition)}</span>
            </div>
            <Slider
              value={[maxPosition]}
              onValueChange={([v]) => setMaxPosition(v)}
              min={10}
              max={500}
              step={10}
            />
            <p className="text-xs text-muted-foreground">
              Maximum size for any single copied position
            </p>
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={handleConfirm}>
            {tier === 'active' ? 'Add to Active 5' : 'Add to Bench'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
