'use client';

import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
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
} from 'lucide-react';
import api from '@/lib/api';
import type { AutoSetupConfig, WorkspaceAllocation } from '@/types/api';

interface AutoSetupStepProps {
  onComplete: () => void;
  onBack: () => void;
}

export function AutoSetupStep({ onComplete, onBack }: AutoSetupStepProps) {
  const queryClient = useQueryClient();
  const [config, setConfig] = useState<AutoSetupConfig>({
    min_roi_30d: 0.05, // 5%
    min_sharpe: 1.0,
    min_win_rate: 0.55, // 55%
    min_trades_30d: 10,
  });
  const [hasRun, setHasRun] = useState(false);
  const [selectedWallets, setSelectedWallets] = useState<string[]>([]);

  // Fetch current allocations
  const { data: allocations } = useQuery({
    queryKey: ['allocations'],
    queryFn: () => api.listAllocations(),
  });

  const autoSetupMutation = useMutation({
    mutationFn: () => api.runAutoSetup(config),
    onSuccess: (data) => {
      setSelectedWallets(data.selected_wallets);
      setHasRun(true);
      queryClient.invalidateQueries({ queryKey: ['allocations'] });
    },
  });

  const formatAddress = (address: string) => {
    return `${address.slice(0, 6)}...${address.slice(-4)}`;
  };

  const activeAllocations = allocations?.filter((a) => a.tier === 'active') ?? [];

  return (
    <div className="space-y-6">
      <div className="text-center space-y-2">
        <h2 className="text-2xl font-bold">Automatic Setup</h2>
        <p className="text-muted-foreground">
          Configure criteria and let the system select the best wallets
        </p>
      </div>

      {!hasRun ? (
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
          <div className="text-center">
            <Button
              size="lg"
              onClick={() => autoSetupMutation.mutate()}
              disabled={autoSetupMutation.isPending}
            >
              {autoSetupMutation.isPending ? (
                <>
                  <Loader2 className="mr-2 h-5 w-5 animate-spin" />
                  Analyzing Wallets...
                </>
              ) : (
                <>
                  <Wand2 className="mr-2 h-5 w-5" />
                  Run Auto-Setup
                </>
              )}
            </Button>
            <p className="text-sm text-muted-foreground mt-2">
              This will analyze top wallets and select the best 5
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
                {selectedWallets.length} wallets selected for your Active 5
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
        <Button variant="outline" onClick={onBack}>
          <ArrowLeft className="mr-2 h-4 w-4" />
          Back
        </Button>
        <Button onClick={onComplete} disabled={!hasRun}>
          <Check className="mr-2 h-4 w-4" />
          Complete Setup
        </Button>
      </div>
    </div>
  );
}
