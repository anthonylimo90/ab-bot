'use client';

import { useState, useMemo } from 'react';
import { useRouter } from 'next/navigation';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Slider } from '@/components/ui/slider';
import { useWalletsQuery, useRecommendationsQuery } from '@/hooks/queries/useWalletsQuery';
import { useModeStore } from '@/stores/mode-store';
import { useWalletStore, selectHasConnectedWallet } from '@/stores/wallet-store';
import { useDemoPortfolioStore } from '@/stores/demo-portfolio-store';
import { formatCurrency, shortenAddress } from '@/lib/utils';
import { ChevronLeft, ChevronRight, Check, Rocket, TestTube2, Wallet, CheckCircle2, ArrowRight } from 'lucide-react';
import { ConnectWalletModal } from '@/components/wallet/ConnectWalletModal';

interface Strategy {
  id: string;
  type: 'WALLET' | 'ARBITRAGE';
  address?: string;
  roi: number;
  sharpe: number;
  recommended: boolean;
  label?: string;
}

const QUICK_AMOUNTS = [25, 50, 100, 500];

export default function AllocatePage() {
  const router = useRouter();
  const [step, setStep] = useState(1);
  const [budget, setBudget] = useState(100);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [allocations, setAllocations] = useState<Record<string, number>>({});
  const [isActivating, setIsActivating] = useState(false);
  const [isActivated, setIsActivated] = useState(false);
  const [showConnectModal, setShowConnectModal] = useState(false);

  // Mode and wallet state
  const { mode } = useModeStore();
  const hasConnectedWallet = useWalletStore(selectHasConnectedWallet);
  const { addPosition, balance: demoBalance } = useDemoPortfolioStore();
  const isDemo = mode === 'demo';

  // Fetch wallets and recommendations from API
  const { data: trackedWallets = [] } = useWalletsQuery();
  const { data: recommendedWallets = [] } = useRecommendationsQuery();

  // Build strategies list from live data
  const strategies: Strategy[] = useMemo(() => {
    const result: Strategy[] = [];

    // Add tracked wallets (TrackedWallet has win_rate and total_pnl directly)
    trackedWallets.forEach((w) => {
      result.push({
        id: w.address,
        type: 'WALLET',
        address: w.address,
        roi: w.win_rate, // Use win_rate as a proxy for performance
        sharpe: w.success_score / 50, // Convert score to approximate Sharpe
        recommended: w.copy_enabled || false,
      });
    });

    // Add recommended wallets that aren't already tracked
    recommendedWallets.forEach((w) => {
      if (!result.some((r) => r.address === w.address)) {
        result.push({
          id: w.address,
          type: 'WALLET',
          address: w.address,
          roi: w.roi_30d,
          sharpe: w.sharpe_ratio,
          recommended: w.confidence > 70,
        });
      }
    });

    // Add arbitrage strategy
    result.push({
      id: 'arbitrage',
      type: 'ARBITRAGE',
      roi: 12.0,
      sharpe: 3.2,
      recommended: true,
      label: 'Arbitrage Bot',
    });

    return result;
  }, [trackedWallets, recommendedWallets]);

  const selectedStrategies = strategies.filter((s) =>
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

  // Handle activation
  const handleActivate = async () => {
    if (isDemo) {
      // Demo mode: Create demo positions and update balance
      setIsActivating(true);

      // Simulate a brief delay for UX
      await new Promise((resolve) => setTimeout(resolve, 1000));

      // Create demo positions for each selected strategy
      selectedStrategies.forEach((strategy) => {
        const allocation = allocations[strategy.id] || 0;
        const amount = budget * (allocation / 100);

        if (strategy.type === 'WALLET' && strategy.address) {
          // Create a position for wallet copy trading
          // Simulate buying YES shares at a reasonable price
          const entryPrice = 0.55 + Math.random() * 0.2; // Random price between 0.55-0.75
          const quantity = Math.floor(amount / entryPrice);

          addPosition({
            walletAddress: strategy.address,
            walletLabel: strategy.label,
            marketId: `market-${strategy.id.slice(0, 8)}`,
            marketQuestion: `Copy trade from ${shortenAddress(strategy.address)}`,
            outcome: 'yes',
            quantity,
            entryPrice,
            currentPrice: entryPrice,
            openedAt: new Date().toISOString(),
          });
        } else if (strategy.type === 'ARBITRAGE') {
          // Create an arbitrage position
          const entryPrice = 0.48;
          const quantity = Math.floor(amount / entryPrice);

          addPosition({
            walletAddress: 'arbitrage-bot',
            walletLabel: 'Arbitrage Bot',
            marketId: 'arb-strategy',
            marketQuestion: 'Arbitrage Strategy Position',
            outcome: 'yes',
            quantity,
            entryPrice,
            currentPrice: entryPrice,
            openedAt: new Date().toISOString(),
          });
        }
      });

      // Note: Balance is already updated in addPosition via the store
      // But we allocated the full budget, so positions cost less than budget
      // The remaining goes back to balance (handled by store)

      setIsActivating(false);
      setIsActivated(true);
    } else {
      // Live mode: Check if wallet is connected
      if (!hasConnectedWallet) {
        setShowConnectModal(true);
        return;
      }

      // TODO: Call live API to activate allocation
      setIsActivating(true);
      await new Promise((resolve) => setTimeout(resolve, 1000));
      setIsActivating(false);
      setIsActivated(true);
    }
  };

  // If activated, show success screen
  if (isActivated) {
    return (
      <div className="max-w-3xl mx-auto space-y-6">
        <Card className="border-profit/20 bg-profit/5">
          <CardContent className="py-12 text-center space-y-6">
            <div className="flex justify-center">
              <div className="h-16 w-16 rounded-full bg-profit/20 flex items-center justify-center">
                <CheckCircle2 className="h-10 w-10 text-profit" />
              </div>
            </div>

            <div className="space-y-2">
              <h2 className="text-2xl font-bold">Allocation Activated!</h2>
              <p className="text-muted-foreground">
                {isDemo ? (
                  <>Your demo allocation of <strong>{formatCurrency(budget)}</strong> is now active.</>
                ) : (
                  <>Your allocation of <strong>{formatCurrency(budget)}</strong> has been activated.</>
                )}
              </p>
            </div>

            {isDemo && (
              <div className="inline-flex items-center gap-2 px-4 py-2 rounded-full bg-demo/10 text-demo text-sm">
                <TestTube2 className="h-4 w-4" />
                Demo Mode - No real funds used
              </div>
            )}

            <div className="border rounded-lg p-4 bg-background/50 text-left">
              <p className="text-sm font-medium mb-3">Allocation Summary:</p>
              <div className="space-y-2">
                {selectedStrategies.map((strategy) => {
                  const allocation = allocations[strategy.id] || 0;
                  const amount = budget * (allocation / 100);
                  return (
                    <div key={strategy.id} className="flex justify-between text-sm">
                      <span className="text-muted-foreground">
                        {strategy.type === 'WALLET'
                          ? shortenAddress(strategy.address!)
                          : strategy.label}
                      </span>
                      <span className="tabular-nums font-medium">
                        {formatCurrency(amount)} ({allocation}%)
                      </span>
                    </div>
                  );
                })}
              </div>
            </div>

            <div className="flex gap-3 justify-center">
              <Button variant="outline" onClick={() => {
                setIsActivated(false);
                setStep(1);
                setSelectedIds([]);
                setAllocations({});
                setBudget(100);
              }}>
                Create Another
              </Button>
              <Button onClick={() => router.push('/portfolio')}>
                View Portfolio
                <ArrowRight className="ml-2 h-4 w-4" />
              </Button>
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

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
            {isDemo && (
              <div className="text-center text-sm text-muted-foreground">
                Available demo balance: <span className="font-medium text-foreground">{formatCurrency(demoBalance)}</span>
              </div>
            )}

            <div className="flex items-center justify-center">
              <div className="relative">
                <span className="absolute left-4 top-1/2 -translate-y-1/2 text-2xl text-muted-foreground">
                  $
                </span>
                <input
                  type="number"
                  value={budget}
                  onChange={(e) => setBudget(Number(e.target.value))}
                  max={isDemo ? demoBalance : undefined}
                  className={`w-48 rounded-lg border bg-background px-4 py-3 pl-10 text-3xl font-bold text-center ${
                    isDemo && budget > demoBalance ? 'border-destructive text-destructive' : ''
                  }`}
                />
              </div>
            </div>

            {isDemo && budget > demoBalance && (
              <p className="text-center text-sm text-destructive">
                Amount exceeds available demo balance
              </p>
            )}

            <div className="flex items-center justify-center gap-2">
              <span className="text-sm text-muted-foreground">Quick amounts:</span>
              {QUICK_AMOUNTS.filter(a => !isDemo || a <= demoBalance).map((amount) => (
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
            {strategies.map((strategy) => {
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
          </CardContent>
        </Card>
      )}

      {/* Mode indicator */}
      {isDemo && (
        <div className="flex items-center justify-center">
          <div className="inline-flex items-center gap-2 px-4 py-2 rounded-full bg-demo/10 text-demo text-sm">
            <TestTube2 className="h-4 w-4" />
            Demo Mode - Using simulated funds
          </div>
        </div>
      )}

      {!isDemo && !hasConnectedWallet && (
        <div className="flex items-center justify-center">
          <div className="inline-flex items-center gap-2 px-4 py-2 rounded-full bg-amber-500/10 text-amber-500 text-sm">
            <Wallet className="h-4 w-4" />
            Connect a wallet to activate live trading
          </div>
        </div>
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
          <Button
            onClick={handleActivate}
            disabled={isActivating || (isDemo && budget > demoBalance)}
            className={isDemo ? 'bg-demo hover:bg-demo/90' : ''}
          >
            {isActivating ? (
              <>
                <div className="mr-2 h-4 w-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                Activating...
              </>
            ) : (
              <>
                <Rocket className="mr-2 h-4 w-4" />
                {isDemo ? 'Activate Demo' : 'Activate'}
              </>
            )}
          </Button>
        )}
      </div>

      {/* Connect Wallet Modal for live mode */}
      <ConnectWalletModal open={showConnectModal} onOpenChange={setShowConnectModal} />
    </div>
  );
}
