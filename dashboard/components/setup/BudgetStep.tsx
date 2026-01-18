'use client';

import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Slider } from '@/components/ui/slider';
import { ArrowLeft, ArrowRight, DollarSign, Info } from 'lucide-react';

interface BudgetStepProps {
  budget: number;
  reservedPct: number;
  onBudgetChange: (value: number) => void;
  onReservedPctChange: (value: number) => void;
  onNext: () => void;
  onBack: () => void;
  isLoading: boolean;
}

export function BudgetStep({
  budget,
  reservedPct,
  onBudgetChange,
  onReservedPctChange,
  onNext,
  onBack,
  isLoading,
}: BudgetStepProps) {
  const tradingCapital = budget * ((100 - reservedPct) / 100);
  const reservedAmount = budget * (reservedPct / 100);

  // Example allocations for different wallet counts
  const exampleAllocations = [
    { wallets: 5, perWallet: tradingCapital / 5 },
    { wallets: 3, perWallet: tradingCapital / 3 },
    { wallets: 1, perWallet: tradingCapital },
  ];

  return (
    <div className="space-y-6">
      <div className="text-center space-y-2">
        <h2 className="text-2xl font-bold">Set Your Trading Budget</h2>
        <p className="text-muted-foreground">
          Define how much capital you want to allocate for copy trading
        </p>
      </div>

      <div className="space-y-6">
        {/* Total Budget */}
        <div className="space-y-2">
          <Label htmlFor="budget">Total Budget (USDC)</Label>
          <div className="relative">
            <DollarSign className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              id="budget"
              type="number"
              min={0}
              step={100}
              value={budget}
              onChange={(e) => onBudgetChange(Number(e.target.value))}
              className="pl-10 text-lg"
              placeholder="10000"
            />
          </div>
          <p className="text-sm text-muted-foreground">
            Enter the total amount you want to allocate for copy trading
          </p>
        </div>

        {/* Reserved Cash */}
        <div className="space-y-4">
          <div className="flex justify-between items-center">
            <Label className="flex items-center gap-2">
              Reserved Cash ({reservedPct}%)
            </Label>
            <span className="text-sm font-medium">
              ${reservedAmount.toLocaleString()}
            </span>
          </div>
          <Slider
            value={[reservedPct]}
            onValueChange={([value]) => onReservedPctChange(value)}
            min={0}
            max={50}
            step={5}
          />
          <p className="text-sm text-muted-foreground">
            Keep {reservedPct}% aside for opportunities or emergencies
          </p>
        </div>

        {/* Trading Capital Summary */}
        <div className="rounded-lg border bg-muted/30 p-4 space-y-4">
          <div className="flex items-center justify-between">
            <h3 className="font-medium">Trading Capital</h3>
            <span className="text-xl font-bold text-green-600">
              ${tradingCapital.toLocaleString()}
            </span>
          </div>

          <p className="text-sm text-muted-foreground">
            This will be split among your active wallets. You&apos;ll select 1-5 wallets in the next step.
          </p>

          {/* Example Allocations */}
          <div className="pt-3 border-t space-y-2">
            <p className="text-xs text-muted-foreground font-medium flex items-center gap-1">
              <Info className="h-3 w-3" />
              Example allocations based on wallet count:
            </p>
            <div className="grid grid-cols-3 gap-2 text-sm">
              {exampleAllocations.map(({ wallets, perWallet }) => (
                <div key={wallets} className="text-center p-2 rounded bg-background">
                  <p className="text-muted-foreground text-xs">{wallets} wallet{wallets !== 1 ? 's' : ''}</p>
                  <p className="font-medium">${perWallet.toLocaleString(undefined, { maximumFractionDigits: 0 })}</p>
                </div>
              ))}
            </div>
          </div>
        </div>
      </div>

      {/* Navigation */}
      <div className="flex justify-between pt-4">
        <Button variant="outline" onClick={onBack} disabled={isLoading}>
          <ArrowLeft className="mr-2 h-4 w-4" />
          Back
        </Button>
        <Button onClick={onNext} disabled={isLoading || budget <= 0}>
          {isLoading ? 'Saving...' : 'Continue'}
          <ArrowRight className="ml-2 h-4 w-4" />
        </Button>
      </div>
    </div>
  );
}
