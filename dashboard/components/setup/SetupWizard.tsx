'use client';

import { useState } from 'react';
import { useRouter } from 'next/navigation';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Progress } from '@/components/ui/progress';
import { CheckCircle, ArrowRight, ArrowLeft, Wand2, Settings2 } from 'lucide-react';
import { BudgetStep } from './BudgetStep';
import { WalletSelectionStep } from './WalletSelectionStep';
import { AutoSetupStep } from './AutoSetupStep';
import api from '@/lib/api';
import type { SetupMode, OnboardingStatus } from '@/types/api';

type WizardStep = 'mode' | 'budget' | 'wallets' | 'auto' | 'complete';

interface SetupWizardProps {
  initialStatus: OnboardingStatus;
}

export function SetupWizard({ initialStatus }: SetupWizardProps) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const [step, setStep] = useState<WizardStep>('mode');
  const [mode, setMode] = useState<SetupMode>(initialStatus.setup_mode);
  const [budget, setBudget] = useState(initialStatus.total_budget);
  const [reservedPct, setReservedPct] = useState(20);

  const setModeMutation = useMutation({
    mutationFn: (newMode: SetupMode) => api.setOnboardingMode(newMode),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['onboarding', 'status'] });
    },
  });

  const setBudgetMutation = useMutation({
    mutationFn: () => api.setOnboardingBudget({ total_budget: budget, reserved_cash_pct: reservedPct }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['onboarding', 'status'] });
    },
  });

  const completeMutation = useMutation({
    mutationFn: () => api.completeOnboarding(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['onboarding', 'status'] });
      router.push('/');
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
      setStep(mode === 'automatic' ? 'auto' : 'wallets');
    } catch {
      // Error handled by mutation
    }
  };

  const handleWalletsComplete = () => {
    setStep('complete');
  };

  const handleAutoComplete = () => {
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
      case 'mode':
        return 20;
      case 'budget':
        return 40;
      case 'wallets':
      case 'auto':
        return 70;
      case 'complete':
        return 100;
    }
  };

  const renderStep = () => {
    switch (step) {
      case 'mode':
        return (
          <div className="space-y-6">
            <div className="text-center space-y-2">
              <h2 className="text-2xl font-bold">Choose Your Setup Mode</h2>
              <p className="text-muted-foreground">
                How would you like to configure your wallet roster?
              </p>
            </div>

            <div className="grid gap-4 md:grid-cols-2">
              <Card
                className="cursor-pointer transition-all hover:border-primary"
                onClick={() => handleModeSelect('manual')}
              >
                <CardHeader>
                  <Settings2 className="h-10 w-10 text-primary mb-2" />
                  <CardTitle>Manual Setup</CardTitle>
                  <CardDescription>
                    Browse discovered wallets, select your favorites, and configure allocations
                    yourself.
                  </CardDescription>
                </CardHeader>
                <CardContent>
                  <ul className="text-sm text-muted-foreground space-y-1">
                    <li>- Full control over wallet selection</li>
                    <li>- Custom allocation percentages</li>
                    <li>- Manual bench management</li>
                  </ul>
                </CardContent>
              </Card>

              <Card
                className="cursor-pointer transition-all hover:border-primary"
                onClick={() => handleModeSelect('automatic')}
              >
                <CardHeader>
                  <Wand2 className="h-10 w-10 text-primary mb-2" />
                  <CardTitle>Automatic Setup</CardTitle>
                  <CardDescription>
                    Let the system analyze and select the best performing wallets using backtesting.
                  </CardDescription>
                </CardHeader>
                <CardContent>
                  <ul className="text-sm text-muted-foreground space-y-1">
                    <li>- AI-powered wallet selection</li>
                    <li>- Optimal allocation calculation</li>
                    <li>- Continuous auto-optimization</li>
                  </ul>
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

      case 'wallets':
        return (
          <WalletSelectionStep
            onComplete={handleWalletsComplete}
            onBack={() => setStep('budget')}
          />
        );

      case 'auto':
        return (
          <AutoSetupStep
            onComplete={handleAutoComplete}
            onBack={() => setStep('budget')}
          />
        );

      case 'complete':
        return (
          <div className="text-center space-y-6">
            <CheckCircle className="h-16 w-16 text-green-500 mx-auto" />
            <div className="space-y-2">
              <h2 className="text-2xl font-bold">Setup Complete!</h2>
              <p className="text-muted-foreground">
                Your workspace is configured and ready to use.
              </p>
            </div>
            <Button size="lg" onClick={handleFinish} disabled={completeMutation.isPending}>
              {completeMutation.isPending ? 'Finishing...' : 'Go to Dashboard'}
              <ArrowRight className="ml-2 h-4 w-4" />
            </Button>
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
