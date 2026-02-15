'use client';

import { useState } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Slider } from '@/components/ui/slider';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { formatCurrency, cn } from '@/lib/utils';
import { Settings, Info } from 'lucide-react';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { useModeStore } from '@/stores/mode-store';
import { useUpdateAllocationMutation } from '@/hooks/queries/useAllocationsQuery';
import type { CopyBehavior, WorkspaceAllocation } from '@/types/api';

interface WalletAllocationSectionProps {
  walletAddress: string;
  totalBalance: number;
  positionsValue: number;
  isDemo?: boolean;
  allocations: WorkspaceAllocation[];
  readOnly?: boolean;
}

const copyBehaviorDescriptions: Record<CopyBehavior, string> = {
  copy_all: 'Copy all trades from this wallet regardless of type',
  events_only: 'Only copy event-based trades (directional bets)',
  arb_threshold: 'Only copy trades meeting arbitrage threshold criteria',
};

export function WalletAllocationSection({
  walletAddress,
  totalBalance,
  positionsValue,
  isDemo = false,
  allocations,
  readOnly = false,
}: WalletAllocationSectionProps) {
  const { currentWorkspace } = useWorkspaceStore();
  const { mode } = useModeStore();
  const updateAllocationMutation = useUpdateAllocationMutation(currentWorkspace?.id, mode);

  // Local state for optimistic slider display during drag (must be before early return)
  const [localAllocationPct, setLocalAllocationPct] = useState<number | null>(null);
  const [localMaxPosition, setLocalMaxPosition] = useState<number | null>(null);

  // Find wallet in roster
  const wallet = allocations.find(
    (w) => w.wallet_address.toLowerCase() === walletAddress.toLowerCase()
  );

  // Not in roster - don't show allocation section
  if (!wallet) {
    return null;
  }

  // Only show for active wallets or bench wallets in readOnly mode
  if (wallet.tier !== 'active' && !readOnly) {
    return null;
  }

  // Calculate allocation values
  const allocationPct = wallet.allocation_pct;
  const maxAllocation = (allocationPct / 100) * totalBalance;
  const inUse = positionsValue;
  const available = Math.max(0, maxAllocation - inUse);

  const displayAllocationPct = localAllocationPct ?? allocationPct;
  const displayMaxAllocation = (displayAllocationPct / 100) * totalBalance;

  const displayMaxPosition = localMaxPosition ?? (wallet.max_position_size ?? 100);

  // Safe division helpers
  const usagePct = maxAllocation > 0 ? Math.min(100, (inUse / maxAllocation) * 100) : 0;
  const remainingPct = maxAllocation > 0 ? Math.max(0, 100 - usagePct) : 100;

  const handleAllocationCommit = (value: number[]) => {
    setLocalAllocationPct(null);
    updateAllocationMutation.mutate({
      address: walletAddress,
      params: { allocation_pct: value[0] },
    });
  };

  const handleBehaviorChange = (value: CopyBehavior) => {
    updateAllocationMutation.mutate({
      address: walletAddress,
      params: { copy_behavior: value },
    });
  };

  const handleMaxPositionCommit = (value: number[]) => {
    setLocalMaxPosition(null);
    updateAllocationMutation.mutate({
      address: walletAddress,
      params: { max_position_size: value[0] },
    });
  };

  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center gap-2 text-lg">
          <Settings className="h-5 w-5" />
          Allocation Settings
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-6">
        {readOnly && (
          <div className="text-sm text-muted-foreground bg-muted/50 rounded-md px-3 py-2">
            Preview of default allocation settings. Promote to Active to customize.
          </div>
        )}
        {/* Allocation Summary Bar */}
        <div className="p-4 bg-muted/30 rounded-lg border">
          <div className="flex items-center justify-between text-sm mb-3">
            <span className="text-muted-foreground">Allocation Overview</span>
            <span className="font-medium">{allocationPct}% of total balance</span>
          </div>
          <div className="grid grid-cols-4 gap-4 text-center">
            <div>
              <p className="text-xs text-muted-foreground mb-1">Allocation</p>
              <p className="text-lg font-bold">{allocationPct}%</p>
            </div>
            <div>
              <p className="text-xs text-muted-foreground mb-1">Max</p>
              <p className="text-lg font-bold">{formatCurrency(maxAllocation)}</p>
            </div>
            <div>
              <p className="text-xs text-muted-foreground mb-1">In Use</p>
              <p className={cn(
                'text-lg font-bold',
                inUse > 0 ? 'text-primary' : 'text-muted-foreground'
              )}>
                {formatCurrency(inUse)}
              </p>
            </div>
            <div>
              <p className="text-xs text-muted-foreground mb-1">Available</p>
              <p className={cn(
                'text-lg font-bold',
                available > 0 ? 'text-profit' : 'text-loss'
              )}>
                {formatCurrency(available)}
              </p>
            </div>
          </div>
          {/* Allocation bar visualization */}
          <div className="mt-3 space-y-1">
            <div className="w-full bg-muted rounded-full h-2 overflow-hidden">
              {maxAllocation > 0 ? (
                <div className="h-full flex">
                  <div
                    className="bg-primary h-full transition-all"
                    style={{ width: `${usagePct}%` }}
                  />
                  <div
                    className="bg-primary/30 h-full transition-all"
                    style={{ width: `${remainingPct}%` }}
                  />
                </div>
              ) : (
                <div className="h-full bg-muted" />
              )}
            </div>
            <p className="text-xs text-muted-foreground text-center">
              {maxAllocation <= 0
                ? 'Set allocation percentage to enable trading'
                : inUse > 0
                ? `${usagePct.toFixed(1)}% of allocation in use`
                : 'No positions open'}
            </p>
          </div>
        </div>

        {/* Settings Grid */}
        <div className="grid md:grid-cols-2 gap-6">
          {/* Allocation Slider */}
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <label className="text-sm font-medium">Allocation Percentage</label>
                <TooltipProvider>
                  <Tooltip>
                    <TooltipTrigger>
                      <Info className="h-3.5 w-3.5 text-muted-foreground" />
                    </TooltipTrigger>
                    <TooltipContent>
                      <p className="max-w-xs">
                        Maximum percentage of your {isDemo ? 'demo' : 'total'} balance
                        that can be allocated to positions from this wallet.
                      </p>
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              </div>
              <span className="text-sm font-medium tabular-nums">
                {displayAllocationPct}%
              </span>
            </div>
            <Slider
              value={[displayAllocationPct]}
              onValueChange={(v) => setLocalAllocationPct(v[0])}
              onValueCommit={handleAllocationCommit}
              min={0}
              max={100}
              step={5}
              className="w-full"
              disabled={readOnly}
            />
            <p className="text-xs text-muted-foreground">
              Max: {formatCurrency(displayMaxAllocation)} of {formatCurrency(totalBalance)}
            </p>
          </div>

          {/* Max Position Size */}
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <label className="text-sm font-medium">Max Position Size</label>
                <TooltipProvider>
                  <Tooltip>
                    <TooltipTrigger>
                      <Info className="h-3.5 w-3.5 text-muted-foreground" />
                    </TooltipTrigger>
                    <TooltipContent>
                      <p className="max-w-xs">
                        Maximum size for any single position copied from this wallet.
                      </p>
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              </div>
              <span className="text-sm font-medium tabular-nums">
                ${displayMaxPosition}
              </span>
            </div>
            <Slider
              value={[displayMaxPosition]}
              onValueChange={(v) => setLocalMaxPosition(v[0])}
              onValueCommit={handleMaxPositionCommit}
              min={10}
              max={500}
              step={10}
              className="w-full"
              disabled={readOnly}
            />
          </div>
        </div>

        {/* Copy Behavior */}
        <div className="space-y-3">
          <div className="flex items-center gap-2">
            <label className="text-sm font-medium">Copy Behavior</label>
            <TooltipProvider>
              <Tooltip>
                <TooltipTrigger>
                  <Info className="h-3.5 w-3.5 text-muted-foreground" />
                </TooltipTrigger>
                <TooltipContent>
                  <p className="max-w-xs">
                    Choose which types of trades to copy from this wallet.
                  </p>
                </TooltipContent>
              </Tooltip>
            </TooltipProvider>
          </div>
          <Select
            value={wallet.copy_behavior}
            onValueChange={(v) => handleBehaviorChange(v as CopyBehavior)}
            disabled={readOnly}
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
          <p className="text-xs text-muted-foreground">
            {copyBehaviorDescriptions[wallet.copy_behavior]}
          </p>
        </div>
      </CardContent>
    </Card>
  );
}
