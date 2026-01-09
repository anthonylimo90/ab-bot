'use client';

import { useState } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Slider } from '@/components/ui/slider';
import { useModeStore } from '@/stores/mode-store';
import { formatCurrency, shortenAddress } from '@/lib/utils';
import { ChevronLeft, ChevronRight, Check, Rocket } from 'lucide-react';

// Mock data
const mockStrategies = [
  {
    id: 'wallet-1',
    type: 'WALLET' as const,
    address: '0x1234567890abcdef1234567890abcdef12345678',
    roi: 47.3,
    sharpe: 2.4,
    recommended: true,
  },
  {
    id: 'wallet-2',
    type: 'WALLET' as const,
    address: '0xabcdef1234567890abcdef1234567890abcdef12',
    roi: 38.1,
    sharpe: 1.9,
    recommended: true,
  },
  {
    id: 'arbitrage',
    type: 'ARBITRAGE' as const,
    roi: 12.0,
    sharpe: 3.2,
    recommended: true,
    label: 'Arbitrage Bot',
  },
  {
    id: 'wallet-3',
    type: 'WALLET' as const,
    address: '0x5678901234abcdef5678901234abcdef56789012',
    roi: 29.4,
    sharpe: 1.5,
    recommended: false,
  },
  {
    id: 'wallet-4',
    type: 'WALLET' as const,
    address: '0x9999888877776666555544443333222211110000',
    roi: 22.1,
    sharpe: 1.2,
    recommended: false,
  },
];

const QUICK_AMOUNTS = [25, 50, 100, 500];

export default function AllocatePage() {
  const { mode, demoBalance } = useModeStore();
  const isDemo = mode === 'demo';

  const [step, setStep] = useState(1);
  const [budget, setBudget] = useState(100);
  const [selectedIds, setSelectedIds] = useState<string[]>(['wallet-1', 'wallet-2', 'arbitrage']);
  const [allocations, setAllocations] = useState<Record<string, number>>({
    'wallet-1': 40,
    'wallet-2': 30,
    'arbitrage': 30,
  });

  const selectedStrategies = mockStrategies.filter((s) =>
    selectedIds.includes(s.id)
  );

  const totalAllocation = Object.values(allocations).reduce((a, b) => a + b, 0);

  const toggleStrategy = (id: string) => {
    if (selectedIds.includes(id)) {
      setSelectedIds(selectedIds.filter((i) => i !== id));
      const newAllocations = { ...allocations };
      delete newAllocations[id];
      setAllocations(newAllocations);
    } else if (selectedIds.length < 5) {
      setSelectedIds([...selectedIds, id]);
      // Distribute equally
      const count = selectedIds.length + 1;
      const equalShare = Math.floor(100 / count);
      const newAllocations: Record<string, number> = {};
      [...selectedIds, id].forEach((sid) => {
        newAllocations[sid] = equalShare;
      });
      setAllocations(newAllocations);
    }
  };

  const updateAllocation = (id: string, value: number) => {
    setAllocations({ ...allocations, [id]: value });
  };

  const distributeEqually = () => {
    const count = selectedIds.length;
    const equalShare = Math.floor(100 / count);
    const newAllocations: Record<string, number> = {};
    selectedIds.forEach((id, index) => {
      // Give remainder to first
      newAllocations[id] = index === 0 ? equalShare + (100 - equalShare * count) : equalShare;
    });
    setAllocations(newAllocations);
  };

  const calculateExpectedReturn = (roi: number, allocation: number) => {
    return (budget * (allocation / 100) * (roi / 100));
  };

  const totalExpectedReturn = selectedStrategies.reduce((sum, s) => {
    const allocation = allocations[s.id] || 0;
    return sum + calculateExpectedReturn(s.roi, allocation);
  }, 0);

  return (
    <div className="max-w-3xl mx-auto space-y-6">
      {/* Progress */}
      <div className="flex items-center justify-between">
        <h1 className="text-3xl font-bold tracking-tight">Strategy Allocation</h1>
        <span className="text-sm text-muted-foreground">Step {step} of 4</span>
      </div>

      <div className="flex gap-2">
        {[1, 2, 3, 4].map((s) => (
          <div
            key={s}
            className={`h-2 flex-1 rounded-full ${
              s <= step ? 'bg-primary' : 'bg-muted'
            }`}
          />
        ))}
      </div>

      {/* Step 1: Budget */}
      {step === 1 && (
        <Card>
          <CardHeader>
            <CardTitle>How much would you like to allocate?</CardTitle>
          </CardHeader>
          <CardContent className="space-y-6">
            <div className="flex items-center justify-center">
              <div className="relative">
                <span className="absolute left-4 top-1/2 -translate-y-1/2 text-2xl text-muted-foreground">
                  $
                </span>
                <input
                  type="number"
                  value={budget}
                  onChange={(e) => setBudget(Number(e.target.value))}
                  className="w-48 rounded-lg border bg-background px-4 py-3 pl-10 text-3xl font-bold text-center"
                />
              </div>
            </div>

            {isDemo && (
              <p className="text-center text-sm text-demo">
                Demo Mode: Using simulated funds
              </p>
            )}

            <div className="flex items-center justify-center gap-2">
              <span className="text-sm text-muted-foreground">Quick amounts:</span>
              {QUICK_AMOUNTS.map((amount) => (
                <Button
                  key={amount}
                  variant={budget === amount ? 'default' : 'outline'}
                  size="sm"
                  onClick={() => setBudget(amount)}
                >
                  ${amount}
                </Button>
              ))}
            </div>
          </CardContent>
        </Card>
      )}

      {/* Step 2: Select Strategies */}
      {step === 2 && (
        <Card>
          <CardHeader>
            <CardTitle>Select strategies to include (max 5)</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            {mockStrategies.map((strategy) => {
              const isSelected = selectedIds.includes(strategy.id);
              return (
                <button
                  key={strategy.id}
                  onClick={() => toggleStrategy(strategy.id)}
                  className={`w-full flex items-center gap-4 p-4 rounded-lg border transition-colors ${
                    isSelected
                      ? 'border-primary bg-primary/5'
                      : 'border-border hover:border-primary/50'
                  }`}
                >
                  <div
                    className={`h-5 w-5 rounded border flex items-center justify-center ${
                      isSelected
                        ? 'bg-primary border-primary'
                        : 'border-muted-foreground'
                    }`}
                  >
                    {isSelected && <Check className="h-3 w-3 text-primary-foreground" />}
                  </div>
                  <div className="flex-1 text-left">
                    <p className="font-medium">
                      {strategy.type === 'WALLET'
                        ? shortenAddress(strategy.address!)
                        : strategy.label}
                    </p>
                    <p className="text-sm text-muted-foreground">
                      ROI +{strategy.roi}% | Sharpe {strategy.sharpe}
                    </p>
                  </div>
                  {strategy.recommended && (
                    <span className="text-xs bg-demo/10 text-demo px-2 py-1 rounded">
                      Recommended
                    </span>
                  )}
                </button>
              );
            })}

            <p className="text-sm text-muted-foreground text-center">
              Selected: {selectedIds.length} strategies
            </p>
          </CardContent>
        </Card>
      )}

      {/* Step 3: Allocate Percentages */}
      {step === 3 && (
        <Card>
          <CardHeader>
            <CardTitle>
              Allocate {formatCurrency(budget)} across strategies
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-6">
            {selectedStrategies.map((strategy) => {
              const allocation = allocations[strategy.id] || 0;
              const amount = budget * (allocation / 100);
              return (
                <div key={strategy.id} className="space-y-2">
                  <div className="flex items-center justify-between">
                    <span className="font-medium">
                      {strategy.type === 'WALLET'
                        ? shortenAddress(strategy.address!)
                        : strategy.label}
                    </span>
                    <span className="tabular-nums">
                      {allocation}% = {formatCurrency(amount)}
                    </span>
                  </div>
                  <Slider
                    value={[allocation]}
                    onValueChange={([value]) => updateAllocation(strategy.id, value)}
                    max={100}
                    step={5}
                  />
                </div>
              );
            })}

            <div className="flex gap-2">
              <Button variant="outline" size="sm" onClick={distributeEqually}>
                Equal Split
              </Button>
              <Button variant="outline" size="sm" disabled>
                Performance Weighted
              </Button>
              <Button variant="outline" size="sm" disabled>
                Risk Adjusted
              </Button>
            </div>

            <div
              className={`text-center font-medium ${
                totalAllocation === 100 ? 'text-profit' : 'text-loss'
              }`}
            >
              Total: {totalAllocation}% {totalAllocation === 100 && '✓'}
            </div>
          </CardContent>
        </Card>
      )}

      {/* Step 4: Review */}
      {step === 4 && (
        <Card>
          <CardHeader>
            <CardTitle>Review your allocation</CardTitle>
          </CardHeader>
          <CardContent className="space-y-6">
            <p className="text-lg">
              Total Capital: <strong>{formatCurrency(budget)}</strong>
            </p>

            <div className="border rounded-lg overflow-hidden">
              <table className="w-full">
                <thead className="bg-muted/50">
                  <tr>
                    <th className="text-left p-3">Strategy</th>
                    <th className="text-right p-3">Allocation</th>
                    <th className="text-right p-3">Expected Return (30d)</th>
                  </tr>
                </thead>
                <tbody>
                  {selectedStrategies.map((strategy) => {
                    const allocation = allocations[strategy.id] || 0;
                    const amount = budget * (allocation / 100);
                    const expectedReturn = calculateExpectedReturn(
                      strategy.roi,
                      allocation
                    );
                    return (
                      <tr key={strategy.id} className="border-t">
                        <td className="p-3">
                          {strategy.type === 'WALLET'
                            ? shortenAddress(strategy.address!)
                            : strategy.label}
                        </td>
                        <td className="p-3 text-right tabular-nums">
                          {formatCurrency(amount)}
                        </td>
                        <td className="p-3 text-right tabular-nums text-profit">
                          +{formatCurrency(expectedReturn)} (+{strategy.roi}%)
                        </td>
                      </tr>
                    );
                  })}
                  <tr className="border-t font-medium bg-muted/30">
                    <td className="p-3">TOTAL</td>
                    <td className="p-3 text-right tabular-nums">
                      {formatCurrency(budget)}
                    </td>
                    <td className="p-3 text-right tabular-nums text-profit">
                      +{formatCurrency(totalExpectedReturn)} (+
                      {((totalExpectedReturn / budget) * 100).toFixed(1)}%)*
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>

            <p className="text-xs text-muted-foreground">
              * Based on historical performance. Past returns do not guarantee
              future results.
            </p>

            {isDemo && (
              <p className="text-sm text-demo text-center">
                Demo Mode: No real funds will be used
              </p>
            )}
          </CardContent>
        </Card>
      )}

      {/* Navigation */}
      <div className="flex justify-between">
        <Button
          variant="outline"
          onClick={() => setStep(step - 1)}
          disabled={step === 1}
        >
          <ChevronLeft className="mr-2 h-4 w-4" />
          Back
        </Button>

        {step < 4 ? (
          <Button
            onClick={() => setStep(step + 1)}
            disabled={
              (step === 2 && selectedIds.length === 0) ||
              (step === 3 && totalAllocation !== 100)
            }
          >
            Next
            <ChevronRight className="ml-2 h-4 w-4" />
          </Button>
        ) : (
          <Button variant="demo">
            <Rocket className="mr-2 h-4 w-4" />
            Activate
          </Button>
        )}
      </div>
    </div>
  );
}
