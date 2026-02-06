'use client';

import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Progress } from '@/components/ui/progress';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import { Badge } from '@/components/ui/badge';
import {
  ArrowLeft,
  Check,
  Loader2,
  Wand2,
  TrendingUp,
  BarChart3,
  Target,
  Activity,
  CheckCircle,
} from 'lucide-react';
import { useToastStore } from '@/stores/toast-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import api from '@/lib/api';
import type { AutoSetupConfig, WorkspaceAllocation } from '@/types/api';

interface AutoSetupStepProps {
  onComplete: (walletCount: number) => void;
  onBack: () => void;
}

export function AutoSetupStep({ onComplete, onBack }: AutoSetupStepProps) {
  const queryClient = useQueryClient();
  const toast = useToastStore();
  const { currentWorkspace } = useWorkspaceStore();
  const [config, setConfig] = useState<AutoSetupConfig>({
    min_roi_30d: 0.05, // 5%
    min_sharpe: 1.0,
    min_win_rate: 0.55, // 55%
    min_trades_30d: 10,
  });
  const [hasRun, setHasRun] = useState(false);
  const [selectedWallets, setSelectedWallets] = useState<string[]>([]);
  const [analysisProgress, setAnalysisProgress] = useState(0);

  // Fetch current allocations
  const { data: allocations } = useQuery({
    queryKey: ['allocations', 'workspace', currentWorkspace?.id],
    queryFn: () => api.listAllocations(),
    enabled: !!currentWorkspace?.id,
  });

  const autoSetupMutation = useMutation({
    mutationFn: async () => {
      // Simulate progress updates for better UX
      setAnalysisProgress(10);
      await new Promise(resolve => setTimeout(resolve, 300));
      setAnalysisProgress(30);
      const result = await api.runAutoSetup(config);
      setAnalysisProgress(70);
      await new Promise(resolve => setTimeout(resolve, 200));
      setAnalysisProgress(100);
      return result;
    },
    onSuccess: (data) => {
      setSelectedWallets(data.selected_wallets);
      setHasRun(true);
      queryClient.invalidateQueries({ queryKey: ['allocations', 'workspace', currentWorkspace?.id] });
      toast.success(
        'Portfolio optimized',
        `${data.selected_wallets.length} wallets selected for your portfolio`
      );
    },
    onError: (error: Error) => {
      setAnalysisProgress(0);
      toast.error('Optimization failed', error.message);
    },
  });

  const formatAddress = (address: string) => {
    return `${address.slice(0, 6)}...${address.slice(-4)}`;
  };

  const activeAllocations = allocations?.filter((a) => a.tier === 'active') ?? [];

  return (
    <div className="space-y-6">
      <div className="text-center space-y-2">
        <h2 className="text-2xl font-bold">Guided Portfolio Setup</h2>
        <p className="text-muted-foreground">
          Configure criteria and let the system select the best wallets for you
        </p>
      </div>

      {autoSetupMutation.isPending ? (
        /* Progress Indicator During Analysis */
        <Card>
          <CardContent className="py-8">
            <div className="space-y-6 text-center">
              <div className="flex justify-center">
                <div className="relative">
                  <Wand2 className="h-12 w-12 text-primary animate-pulse" />
                </div>
              </div>
              <div>
                <h3 className="text-lg font-medium mb-2">Analyzing Wallets...</h3>
                <Progress value={analysisProgress} className="h-2 max-w-xs mx-auto" />
              </div>
              <ul className="text-sm space-y-2 text-left max-w-xs mx-auto">
                <li className="flex items-center gap-2">
                  {analysisProgress >= 10 ? (
                    <CheckCircle className="h-4 w-4 text-green-500" />
                  ) : (
                    <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
                  )}
                  <span className={analysisProgress >= 10 ? '' : 'text-muted-foreground'}>
                    Fetching wallet candidates
                  </span>
                </li>
                <li className="flex items-center gap-2">
                  {analysisProgress >= 30 ? (
                    <CheckCircle className="h-4 w-4 text-green-500" />
                  ) : analysisProgress >= 10 ? (
                    <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
                  ) : (
                    <div className="h-4 w-4 rounded-full border-2 border-muted-foreground" />
                  )}
                  <span className={analysisProgress >= 30 ? '' : 'text-muted-foreground'}>
                    Analyzing 30-day performance
                  </span>
                </li>
                <li className="flex items-center gap-2">
                  {analysisProgress >= 70 ? (
                    <CheckCircle className="h-4 w-4 text-green-500" />
                  ) : analysisProgress >= 30 ? (
                    <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
                  ) : (
                    <div className="h-4 w-4 rounded-full border-2 border-muted-foreground" />
                  )}
                  <span className={analysisProgress >= 70 ? '' : 'text-muted-foreground'}>
                    Running backtests
                  </span>
                </li>
                <li className="flex items-center gap-2">
                  {analysisProgress >= 100 ? (
                    <CheckCircle className="h-4 w-4 text-green-500" />
                  ) : analysisProgress >= 70 ? (
                    <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
                  ) : (
                    <div className="h-4 w-4 rounded-full border-2 border-muted-foreground" />
                  )}
                  <span className={analysisProgress >= 100 ? '' : 'text-muted-foreground'}>
                    Selecting optimal portfolio
                  </span>
                </li>
              </ul>
            </div>
          </CardContent>
        </Card>
      ) : !hasRun ? (
        <>
          {/* Criteria Configuration */}
          <Card>
            <CardHeader>
              <CardTitle className="text-sm">Selection Criteria</CardTitle>
              <CardDescription>Set minimum thresholds for wallet selection</CardDescription>
            </CardHeader>
            <CardContent className="grid gap-4 md:grid-cols-2">
              <div className="space-y-2">
                <Label className="flex items-center gap-2">
                  <TrendingUp className="h-4 w-4" />
                  Min. ROI (30d)
                </Label>
                <div className="flex items-center gap-2">
                  <Input
                    type="number"
                    min={0}
                    max={100}
                    step={1}
                    value={(config.min_roi_30d ?? 0) * 100}
                    onChange={(e) =>
                      setConfig({ ...config, min_roi_30d: Number(e.target.value) / 100 })
                    }
                    className="w-24"
                  />
                  <span className="text-sm text-muted-foreground">%</span>
                </div>
              </div>

              <div className="space-y-2">
                <Label className="flex items-center gap-2">
                  <BarChart3 className="h-4 w-4" />
                  Min. Sharpe Ratio
                </Label>
                <Input
                  type="number"
                  min={0}
                  step={0.1}
                  value={config.min_sharpe ?? 0}
                  onChange={(e) =>
                    setConfig({ ...config, min_sharpe: Number(e.target.value) })
                  }
                  className="w-24"
                />
              </div>

              <div className="space-y-2">
                <Label className="flex items-center gap-2">
                  <Target className="h-4 w-4" />
                  Min. Win Rate
                </Label>
                <div className="flex items-center gap-2">
                  <Input
                    type="number"
                    min={0}
                    max={100}
                    step={1}
                    value={(config.min_win_rate ?? 0) * 100}
                    onChange={(e) =>
                      setConfig({ ...config, min_win_rate: Number(e.target.value) / 100 })
                    }
                    className="w-24"
                  />
                  <span className="text-sm text-muted-foreground">%</span>
                </div>
              </div>

              <div className="space-y-2">
                <Label className="flex items-center gap-2">
                  <Activity className="h-4 w-4" />
                  Min. Trades (30d)
                </Label>
                <Input
                  type="number"
                  min={0}
                  step={1}
                  value={config.min_trades_30d ?? 0}
                  onChange={(e) =>
                    setConfig({ ...config, min_trades_30d: Number(e.target.value) })
                  }
                  className="w-24"
                />
              </div>
            </CardContent>
          </Card>

          {/* Run Auto-Setup */}
          <div className="text-center space-y-3">
            <Button
              size="lg"
              onClick={() => autoSetupMutation.mutate()}
            >
              <Wand2 className="mr-2 h-5 w-5" />
              Optimize My Portfolio
            </Button>
            <p className="text-sm text-muted-foreground">
              The system will analyze top wallets and select the best performers
            </p>
          </div>
        </>
      ) : (
        <>
          {/* Results */}
          <Card className="border-green-500/30 bg-green-500/5">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-green-600">
                <Check className="h-5 w-5" />
                Auto-Setup Complete
              </CardTitle>
              <CardDescription>
                {selectedWallets.length} wallets selected for your Active portfolio
              </CardDescription>
            </CardHeader>
            <CardContent>
              <div className="space-y-2">
                {activeAllocations.map((allocation, idx) => (
                  <div
                    key={allocation.wallet_address}
                    className="flex items-center justify-between p-3 rounded-lg border bg-background"
                  >
                    <div className="flex items-center gap-3">
                      <Badge variant="outline">#{idx + 1}</Badge>
                      <span className="font-mono text-sm">
                        {formatAddress(allocation.wallet_address)}
                      </span>
                    </div>
                    <div className="flex items-center gap-4 text-sm text-muted-foreground">
                      {allocation.backtest_roi && (
                        <span>ROI: {(allocation.backtest_roi * 100).toFixed(1)}%</span>
                      )}
                      {allocation.backtest_win_rate && (
                        <span>Win: {(allocation.backtest_win_rate * 100).toFixed(1)}%</span>
                      )}
                      <Badge>{allocation.allocation_pct}%</Badge>
                    </div>
                  </div>
                ))}
              </div>
            </CardContent>
          </Card>
        </>
      )}

      {/* Navigation */}
      <div className="flex justify-between pt-4">
        <Button variant="outline" onClick={onBack} disabled={autoSetupMutation.isPending}>
          <ArrowLeft className="mr-2 h-4 w-4" />
          Back
        </Button>
        <Button
          onClick={() => onComplete(activeAllocations.length)}
          disabled={!hasRun || autoSetupMutation.isPending}
        >
          <Check className="mr-2 h-4 w-4" />
          Complete Setup ({activeAllocations.length} wallet{activeAllocations.length !== 1 ? 's' : ''})
        </Button>
      </div>
    </div>
  );
}
