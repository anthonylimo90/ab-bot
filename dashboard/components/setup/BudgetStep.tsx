'use client';

import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Slider } from '@/components/ui/slider';
import { ArrowLeft, ArrowRight, DollarSign } from 'lucide-react';

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
  const allocatedAmount = budget * ((100 - reservedPct) / 100);
  const reservedAmount = budget * (reservedPct / 100);

  return (
    <div className="space-y-6">
      <div className="text-center space-y-2">
        <h2 className="text-2xl font-bold">Set Your Budget</h2>
        <p className="text-muted-foreground">
          Configure your total trading budget and cash reserve
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
          <div className="flex justify-between">
            <Label>Reserved Cash ({reservedPct}%)</Label>
            <span className="text-sm text-muted-foreground">
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
            Cash reserve kept aside for unexpected opportunities or emergencies
          </p>
        </div>

        {/* Summary */}
        <div className="rounded-lg border p-4 space-y-3 bg-muted/30">
          <h3 className="font-medium">Budget Summary</h3>
          <div className="grid grid-cols-2 gap-4 text-sm">
            <div>
              <p className="text-muted-foreground">Total Budget</p>
              <p className="text-lg font-semibold">${budget.toLocaleString()}</p>
            </div>
            <div>
              <p className="text-muted-foreground">Available for Trading</p>
              <p className="text-lg font-semibold text-green-600">
                ${allocatedAmount.toLocaleString()}
              </p>
            </div>
            <div>
              <p className="text-muted-foreground">Reserved Cash</p>
              <p className="text-lg font-semibold">${reservedAmount.toLocaleString()}</p>
            </div>
            <div>
              <p className="text-muted-foreground">Per Wallet (5 wallets)</p>
              <p className="text-lg font-semibold">
                ${(allocatedAmount / 5).toLocaleString()}
              </p>
            </div>
          </div>
        </div>
      </div>

      {/* Navigation */}
      <div className="flex justify-between pt-4">
        <Button variant="outline" onClick={onBack}>
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
