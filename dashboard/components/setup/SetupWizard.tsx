'use client';

import { useState } from 'react';
import { useRouter } from 'next/navigation';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Progress } from '@/components/ui/progress';
import { CheckCircle, ArrowRight, ArrowLeft, Wand2, Settings2, Target, Clock, Eye, Wallet, TrendingUp, BarChart3 } from 'lucide-react';
import { BudgetStep } from './BudgetStep';
import { WalletSelectionStep } from './WalletSelectionStep';
import { AutoSetupStep } from './AutoSetupStep';
import { RiskToleranceStep } from './RiskToleranceStep';
import { AllocationPie } from '@/components/charts/AllocationPie';
import { useAllocationsQuery } from '@/hooks/queries/useAllocationsQuery';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { useToastStore } from '@/stores/toast-store';
import api from '@/lib/api';
import type { SetupMode, OnboardingStatus } from '@/types/api';

type WizardStep = 'mode' | 'budget' | 'risk' | 'wallets' | 'auto' | 'complete';

interface SetupWizardProps {
  initialStatus: OnboardingStatus;
}

export function SetupWizard({ initialStatus }: SetupWizardProps) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const toast = useToastStore();
  const { currentWorkspace } = useWorkspaceStore();
  const [step, setStep] = useState<WizardStep>('mode');
  const [mode, setMode] = useState<SetupMode>(initialStatus.setup_mode || 'automatic');
  const [budget, setBudget] = useState<number>(initialStatus.total_budget || 0);
  const [reservedPct, setReservedPct] = useState(20);
  const [activeWalletCount, setActiveWalletCount] = useState(0);

  const { data: allocations = [] } = useAllocationsQuery(currentWorkspace?.id);

  const setModeMutation = useMutation({
    mutationFn: (newMode: SetupMode) => api.setOnboardingMode(newMode),
    onSuccess: (_data, selectedMode) => {
      queryClient.invalidateQueries({ queryKey: ['onboarding', 'status'] });
      toast.success('Mode selected', `${selectedMode === 'automatic' ? 'Guided' : 'Custom'} mode enabled`);
    },
    onError: (error: Error) => {
      toast.error('Failed to set mode', error.message);
    },
  });

  const setBudgetMutation = useMutation({
    mutationFn: () => api.setOnboardingBudget({ total_budget: budget, reserved_cash_pct: reservedPct }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['onboarding', 'status'] });
      toast.success('Budget configured', 'Your trading budget has been set');
    },
    onError: (error: Error) => {
      toast.error('Failed to set budget', error.message);
    },
  });

  const completeMutation = useMutation({
    mutationFn: () => api.completeOnboarding(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['onboarding', 'status'] });
      toast.success('Setup complete', 'Welcome to AB-Bot!');
      router.push('/');
    },
    onError: (error: Error) => {
      toast.error('Failed to complete setup', error.message);
    },
  });

  const handleModeSelect = async (selectedMode: SetupMode) => {
    setMode(selectedMode);
    try {
      await setModeMutation.mutateAsync(selectedMode);
      setStep('budget');
    } catch {
      // Error handled by mutation
    }
  };

  const handleBudgetNext = async () => {
    try {
      await setBudgetMutation.mutateAsync();
      setStep('risk');
    } catch {
      // Error handled by mutation
    }
  };

  const handleRiskNext = (_preset: import('@/lib/riskPresets').RiskPreset) => {
    setStep(mode === 'automatic' ? 'auto' : 'wallets');
  };

  const handleWalletsComplete = (walletCount: number) => {
    setActiveWalletCount(walletCount);
    setStep('complete');
  };

  const handleAutoComplete = (walletCount: number) => {
    setActiveWalletCount(walletCount);
    setStep('complete');
  };

  const handleFinish = async () => {
    try {
      await completeMutation.mutateAsync();
    } catch {
      // Error handled by mutation
    }
  };

  const getProgress = () => {
    switch (step) {
      case 'mode': return 20;
      case 'budget': return 40;
      case 'risk': return 55;
      case 'wallets':
      case 'auto': return 75;
      case 'complete': return 100;
    }
  };

  const renderStep = () => {
    switch (step) {
      case 'mode':
        return (
          <div className="space-y-6">
            <div className="text-center space-y-2">
              <h2 className="text-2xl font-bold">How do you want to manage your portfolio?</h2>
              <p className="text-muted-foreground">
                Choose the management style that fits your trading approach
              </p>
            </div>

            <div className="grid gap-4 md:grid-cols-2">
              <Card
                className={`cursor-pointer transition-all hover:border-primary hover:shadow-md ${setModeMutation.isPending ? 'opacity-50 pointer-events-none' : ''}`}
                onClick={() => handleModeSelect('automatic')}
              >
                <CardHeader>
                  <div className="flex items-center justify-between">
                    <Target className="h-10 w-10 text-primary" />
                    <span className="text-xs bg-primary/10 text-primary px-2 py-1 rounded-full font-medium">
                      Recommended
                    </span>
                  </div>
                  <CardTitle className="mt-2">Guided</CardTitle>
                  <CardDescription>
                    Best for most users. System analyzes wallets and optimizes your portfolio.
                  </CardDescription>
                </CardHeader>
                <CardContent className="space-y-3">
                  <ul className="text-sm space-y-2">
                    <li className="flex items-center gap-2">
                      <BarChart3 className="h-4 w-4 text-muted-foreground" />
                      System analyzes wallet performance
                    </li>
                    <li className="flex items-center gap-2">
                      <TrendingUp className="h-4 w-4 text-muted-foreground" />
                      Optimizes selection automatically
                    </li>
                    <li className="flex items-center gap-2">
                      <Clock className="h-4 w-4 text-muted-foreground" />
                      Auto-rebalances weekly
                    </li>
                  </ul>
                  <p className="text-xs text-muted-foreground pt-2 border-t">
                    Quick setup - get started in minutes
                  </p>
                </CardContent>
              </Card>

              <Card
                className={`cursor-pointer transition-all hover:border-primary hover:shadow-md ${setModeMutation.isPending ? 'opacity-50 pointer-events-none' : ''}`}
                onClick={() => handleModeSelect('manual')}
              >
                <CardHeader>
                  <Settings2 className="h-10 w-10 text-muted-foreground" />
                  <CardTitle className="mt-2">Custom</CardTitle>
                  <CardDescription>
                    For experienced traders. Full control over wallet selection and allocations.
                  </CardDescription>
                </CardHeader>
                <CardContent className="space-y-3">
                  <ul className="text-sm space-y-2">
                    <li className="flex items-center gap-2">
                      <Eye className="h-4 w-4 text-muted-foreground" />
                      Browse all available wallets
                    </li>
                    <li className="flex items-center gap-2">
                      <Wallet className="h-4 w-4 text-muted-foreground" />
                      Choose your own portfolio
                    </li>
                    <li className="flex items-center gap-2">
                      <Settings2 className="h-4 w-4 text-muted-foreground" />
                      Full manual control
                    </li>
                  </ul>
                  <p className="text-xs text-muted-foreground pt-2 border-t">
                    More setup time - fully customizable
                  </p>
                </CardContent>
              </Card>
            </div>
          </div>
        );

      case 'budget':
        return (
          <BudgetStep
            budget={budget}
            reservedPct={reservedPct}
            onBudgetChange={setBudget}
            onReservedPctChange={setReservedPct}
            onNext={handleBudgetNext}
            onBack={() => setStep('mode')}
            isLoading={setBudgetMutation.isPending}
          />
        );

      case 'risk':
        return (
          <RiskToleranceStep
            onNext={handleRiskNext}
            onBack={() => setStep('budget')}
          />
        );

      case 'wallets':
        return (
          <WalletSelectionStep
            onComplete={handleWalletsComplete}
            onBack={() => setStep('risk')}
          />
        );

      case 'auto':
        return (
          <AutoSetupStep
            onComplete={handleAutoComplete}
            onBack={() => setStep('risk')}
          />
        );

      case 'complete':
        const tradingCapital = budget * ((100 - reservedPct) / 100);
        const reservedAmount = budget * (reservedPct / 100);
        const perWalletAmount = activeWalletCount > 0 ? tradingCapital / activeWalletCount : 0;

        return (
          <div className="space-y-6">
            <div className="text-center space-y-4">
              <div className="flex h-16 w-16 items-center justify-center rounded-full bg-green-100 mx-auto">
                <CheckCircle className="h-10 w-10 text-green-600" />
              </div>
              <div className="space-y-2">
                <h2 className="text-2xl font-bold">You&apos;re Ready to Trade!</h2>
                <p className="text-muted-foreground">
                  Your portfolio has been configured and monitoring has started.
                </p>
              </div>
            </div>

            {/* Portfolio Summary */}
            <div className="rounded-lg border bg-muted/30 p-4 space-y-4">
              <h3 className="font-medium text-sm text-muted-foreground">Your Portfolio Summary</h3>
              <div className="grid grid-cols-2 gap-4">
                <div>
                  <p className="text-sm text-muted-foreground">Total Budget</p>
                  <p className="text-lg font-semibold">${budget.toLocaleString()}</p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Trading Capital</p>
                  <p className="text-lg font-semibold text-green-600">${tradingCapital.toLocaleString()}</p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Reserved Cash</p>
                  <p className="text-lg font-semibold">${reservedAmount.toLocaleString()}</p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Active Wallets</p>
                  <p className="text-lg font-semibold">{activeWalletCount} wallet{activeWalletCount !== 1 ? 's' : ''}</p>
                </div>
              </div>
              {activeWalletCount > 0 && (
                <div className="pt-2 border-t">
                  <p className="text-sm text-muted-foreground">
                    ~${perWalletAmount.toLocaleString(undefined, { maximumFractionDigits: 0 })} allocated per wallet
                  </p>
                </div>
              )}
            </div>

            {/* What Happens Next */}
            <div className="rounded-lg border p-4 space-y-3">
              <h3 className="font-medium">What happens next?</h3>
              <ul className="space-y-3">
                <li className="flex items-start gap-3">
                  <div className="flex h-6 w-6 items-center justify-center rounded-full bg-green-100 text-green-600 text-xs font-medium shrink-0">
                    1
                  </div>
                  <div>
                    <p className="text-sm font-medium">Monitoring has started</p>
                    <p className="text-xs text-muted-foreground">We&apos;re watching your selected wallets for trades</p>
                  </div>
                </li>
                <li className="flex items-start gap-3">
                  <div className="flex h-6 w-6 items-center justify-center rounded-full bg-muted text-muted-foreground text-xs font-medium shrink-0">
                    2
                  </div>
                  <div>
                    <p className="text-sm font-medium">Trades will be copied</p>
                    <p className="text-xs text-muted-foreground">When monitored wallets trade, we&apos;ll mirror their positions</p>
                  </div>
                </li>
                <li className="flex items-start gap-3">
                  <div className="flex h-6 w-6 items-center justify-center rounded-full bg-muted text-muted-foreground text-xs font-medium shrink-0">
                    3
                  </div>
                  <div>
                    <p className="text-sm font-medium">Track your performance</p>
                    <p className="text-xs text-muted-foreground">Check the dashboard daily to monitor your positions</p>
                  </div>
                </li>
              </ul>
            </div>

            {/* Wallet Allocation Preview */}
            {allocations.length > 0 && (
              <Card>
                <CardHeader>
                  <CardTitle className="text-sm">Wallet Allocation</CardTitle>
                </CardHeader>
                <CardContent>
                  <AllocationPie
                    data={allocations
                      .filter((a) => a.tier === 'active')
                      .map((a, i) => ({
                        name: a.wallet_label || `${a.wallet_address.slice(0, 6)}...${a.wallet_address.slice(-4)}`,
                        value: a.allocation_pct,
                        color: ['#3b82f6', '#10b981', '#f59e0b', '#ef4444', '#8b5cf6'][i % 5],
                      }))}
                    showLegend
                  />
                </CardContent>
              </Card>
            )}

            {/* Actions */}
            <div className="flex flex-col sm:flex-row gap-3 pt-2">
              <Button size="lg" className="flex-1" onClick={handleFinish} disabled={completeMutation.isPending}>
                {completeMutation.isPending ? 'Finishing...' : 'Go to Dashboard'}
                <ArrowRight className="ml-2 h-4 w-4" />
              </Button>
            </div>
          </div>
        );
    }
  };

  return (
    <div className="max-w-3xl mx-auto space-y-6">
      {/* Progress */}
      <div className="space-y-2">
        <div className="flex justify-between text-sm text-muted-foreground">
          <span>Setup Progress</span>
          <span>{getProgress()}%</span>
        </div>
        <Progress value={getProgress()} />
      </div>

      {/* Step Content */}
      <Card>
        <CardContent className="pt-6">{renderStep()}</CardContent>
      </Card>
    </div>
  );
}
