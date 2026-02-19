'use client';

import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { formatDistanceToNow } from 'date-fns';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { queryKeys } from '@/lib/queryClient';
import { ratioOrPercentToPercent } from '@/lib/utils';
import { useNotificationStore } from '@/stores/notification-store';
import { useToastStore } from '@/stores/toast-store';
import {
  useAcknowledgeRotationMutation,
  useOptimizerStatusQuery,
  useRotationHistoryQuery,
  useTriggerOptimizationMutation,
} from '@/hooks/queries/useOptimizerQuery';
import { api } from '@/lib/api';
import {
  AlertTriangle,
  Ban,
  Bot,
  CheckCircle,
  Gauge,
  History,
  Info,
  Play,
  RotateCcw,
  Save,
  Settings,
  Shield,
  TrendingDown,
  TrendingUp,
  XCircle,
} from 'lucide-react';
import type { OptimizerStatus, WalletBan } from '@/types/api';

interface OptimizationSettingsDraft {
  auto_optimize_enabled: boolean;
  optimization_interval_hours: number;
  min_roi_30d: number;
  min_sharpe: number;
  min_win_rate: number;
  min_trades_30d: number;
}

interface AutomationPanelProps {
  workspaceId: string;
  onRefresh?: () => void;
}

type RiskPreset = 'conservative' | 'balanced' | 'aggressive';
type OpportunityAggressiveness = 'stable' | 'balanced' | 'discovery';

interface OpportunitySelectionDraft {
  aggressiveness: OpportunityAggressiveness;
  exploration_slots: number;
}

const RISK_PRESETS: Record<
  RiskPreset,
  {
    label: string;
    description: string;
    settings: Pick<
      OptimizationSettingsDraft,
      'optimization_interval_hours' | 'min_roi_30d' | 'min_sharpe' | 'min_win_rate' | 'min_trades_30d'
    >;
  }
> = {
  conservative: {
    label: 'Conservative',
    description: 'Higher quality bar, fewer rotations.',
    settings: {
      optimization_interval_hours: 24,
      min_roi_30d: 8,
      min_sharpe: 1.3,
      min_win_rate: 58,
      min_trades_30d: 20,
    },
  },
  balanced: {
    label: 'Balanced',
    description: 'Moderate quality bar for steady discovery.',
    settings: {
      optimization_interval_hours: 12,
      min_roi_30d: 5,
      min_sharpe: 1,
      min_win_rate: 50,
      min_trades_30d: 10,
    },
  },
  aggressive: {
    label: 'Aggressive',
    description: 'Lower thresholds for broader candidate exploration.',
    settings: {
      optimization_interval_hours: 6,
      min_roi_30d: 2,
      min_sharpe: 0.6,
      min_win_rate: 45,
      min_trades_30d: 5,
    },
  },
};

const OPPORTUNITY_PRESETS: Record<
  OpportunityAggressiveness,
  { label: string; description: string; defaultExplorationSlots: number }
> = {
  stable: {
    label: 'Stable',
    description: 'Prioritize consistency and lower rotation churn.',
    defaultExplorationSlots: 2,
  },
  balanced: {
    label: 'Balanced',
    description: 'Mix steady execution with measured discovery.',
    defaultExplorationSlots: 5,
  },
  discovery: {
    label: 'Discovery',
    description: 'Increase exploration to surface new opportunities faster.',
    defaultExplorationSlots: 8,
  },
};

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

function toSettingsDraft(status: OptimizerStatus): OptimizationSettingsDraft {
  return {
    auto_optimize_enabled: status.enabled,
    optimization_interval_hours: status.interval_hours || 12,
    min_roi_30d: ratioOrPercentToPercent(status.criteria.min_roi_30d),
    min_sharpe: status.criteria.min_sharpe ?? 1,
    min_win_rate: ratioOrPercentToPercent(status.criteria.min_win_rate),
    min_trades_30d: status.criteria.min_trades_30d ?? 10,
  };
}

export function AutomationPanel({ workspaceId, onRefresh }: AutomationPanelProps) {
  const queryClient = useQueryClient();
  const { addToast } = useToastStore();
  const hasWorkspace = Boolean(workspaceId);

  const { data: optimizerStatus, isLoading: isStatusLoading } = useOptimizerStatusQuery(
    hasWorkspace ? workspaceId : undefined
  );
  const { data: history = [] } = useRotationHistoryQuery({
    workspaceId: hasWorkspace ? workspaceId : undefined,
    limit: 20,
  });
  const { data: bansData } = useQuery({
    queryKey: ['wallet-bans', workspaceId],
    queryFn: () => api.listBans(),
    enabled: hasWorkspace,
    staleTime: 30 * 1000,
  });
  const { data: dynamicTunerStatus, isLoading: isTunerLoading } = useQuery({
    queryKey: ['dynamic-tuner-status', workspaceId],
    queryFn: () => api.getDynamicTunerStatus(workspaceId),
    enabled: hasWorkspace,
    staleTime: 15 * 1000,
    refetchInterval: 30 * 1000,
  });
  const bans = bansData?.bans ?? [];

  const triggerOptimizationMutation = useTriggerOptimizationMutation();
  const acknowledgeMutation = useAcknowledgeRotationMutation();
  const saveSettingsMutation = useMutation({
    mutationFn: (settings: OptimizationSettingsDraft) =>
      api.updateWorkspace(workspaceId, {
        auto_optimize_enabled: settings.auto_optimize_enabled,
        optimization_interval_hours: settings.optimization_interval_hours,
        min_roi_30d: settings.min_roi_30d,
        min_sharpe: settings.min_sharpe,
        min_win_rate: settings.min_win_rate,
        min_trades_30d: settings.min_trades_30d,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.optimizer.status(workspaceId) });
      queryClient.invalidateQueries({ queryKey: ['workspace', workspaceId] });
      addToast({ type: 'success', title: 'Automation settings saved' });
    },
    onError: (error: Error) => {
      addToast({ type: 'error', title: 'Failed to save settings', description: error.message });
    },
  });
  const saveOpportunitySelectionMutation = useMutation({
    mutationFn: (selection: OpportunitySelectionDraft) =>
      api.updateOpportunitySelection(workspaceId, selection),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['dynamic-tuner-status', workspaceId] });
      queryClient.invalidateQueries({ queryKey: ['dynamic-tuning'] });
      addToast({ type: 'success', title: 'Opportunity selection settings saved' });
    },
    onError: (error: Error) => {
      addToast({
        type: 'error',
        title: 'Failed to save opportunity selection',
        description: error.message,
      });
    },
  });
  const unbanMutation = useMutation({
    mutationFn: (address: string) => api.unbanWallet(address),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['wallet-bans', workspaceId] });
      addToast({ type: 'success', title: 'Wallet unbanned' });
    },
    onError: () => {
      addToast({ type: 'error', title: 'Failed to unban wallet' });
    },
  });

  const baselineSettings = useMemo(() => {
    if (!optimizerStatus) {
      return null;
    }
    return toSettingsDraft(optimizerStatus);
  }, [optimizerStatus]);

  const [settings, setSettings] = useState<OptimizationSettingsDraft | null>(null);
  const [opportunitySettings, setOpportunitySettings] = useState<OpportunitySelectionDraft | null>(
    null
  );

  useEffect(() => {
    setSettings(null);
    setOpportunitySettings(null);
  }, [workspaceId]);

  useEffect(() => {
    if (!baselineSettings) {
      return;
    }
    setSettings((current) => current ?? baselineSettings);
  }, [baselineSettings]);

  const baselineOpportunitySettings = useMemo<OpportunitySelectionDraft | null>(() => {
    if (!dynamicTunerStatus?.opportunity_selection) {
      return null;
    }

    const rawAggressiveness = dynamicTunerStatus.opportunity_selection.aggressiveness;
    const aggressiveness: OpportunityAggressiveness =
      rawAggressiveness === 'stable' || rawAggressiveness === 'discovery'
        ? rawAggressiveness
        : 'balanced';

    return {
      aggressiveness,
      exploration_slots: Math.max(
        1,
        Math.round(dynamicTunerStatus.opportunity_selection.exploration_slots || 1)
      ),
    };
  }, [dynamicTunerStatus]);

  useEffect(() => {
    if (!baselineOpportunitySettings) {
      return;
    }
    setOpportunitySettings((current) => current ?? baselineOpportunitySettings);
  }, [baselineOpportunitySettings]);

  const hasUnsavedChanges = useMemo(() => {
    if (!settings || !baselineSettings) {
      return false;
    }
    return JSON.stringify(settings) !== JSON.stringify(baselineSettings);
  }, [settings, baselineSettings]);

  const hasOpportunityUnsavedChanges = useMemo(() => {
    if (!opportunitySettings || !baselineOpportunitySettings) {
      return false;
    }
    return JSON.stringify(opportunitySettings) !== JSON.stringify(baselineOpportunitySettings);
  }, [opportunitySettings, baselineOpportunitySettings]);

  const updateSettings = (patch: Partial<OptimizationSettingsDraft>) => {
    setSettings((current) => (current ? { ...current, ...patch } : current));
  };

  const updateOpportunitySettings = (patch: Partial<OpportunitySelectionDraft>) => {
    setOpportunitySettings((current) => (current ? { ...current, ...patch } : current));
  };

  const applyPreset = (preset: RiskPreset) => {
    const presetConfig = RISK_PRESETS[preset];
    setSettings((current) => (current ? { ...current, ...presetConfig.settings } : current));
    addToast({
      type: 'info',
      title: `${presetConfig.label} preset applied`,
      description: presetConfig.description,
    });
  };

  const handleSaveSettings = () => {
    if (!settings || !hasWorkspace) {
      return;
    }
    saveSettingsMutation.mutate(settings);
  };

  const applyOpportunityPresetDefaults = () => {
    if (!opportunitySettings) {
      return;
    }
    const preset = OPPORTUNITY_PRESETS[opportunitySettings.aggressiveness];
    updateOpportunitySettings({ exploration_slots: preset.defaultExplorationSlots });
    addToast({
      type: 'info',
      title: 'Applied recommended defaults',
      description: `${preset.label}: ${preset.defaultExplorationSlots} exploration slots`,
    });
  };

  const handleSaveOpportunitySettings = () => {
    if (!hasWorkspace || !opportunitySettings) {
      return;
    }
    saveOpportunitySelectionMutation.mutate(opportunitySettings);
  };

  const handleTriggerOptimization = async () => {
    try {
      const result = await triggerOptimizationMutation.mutateAsync();
      onRefresh?.();

      if (result.candidates_found === 0) {
        useNotificationStore.getState().noWalletsFound(result.thresholds);
        addToast({
          type: 'info',
          title: 'Optimization complete',
          description: 'No wallets met the current thresholds',
        });
      } else if (result.wallets_promoted > 0) {
        useNotificationStore.getState().optimizationSuccess(result.wallets_promoted);
        addToast({ type: 'success', title: `${result.wallets_promoted} wallet(s) promoted` });
      } else if (result.candidates_found === -1) {
        addToast({ type: 'success', title: 'Optimization triggered successfully' });
      } else {
        addToast({
          type: 'info',
          title: 'Optimization complete',
          description: 'Roster is already full',
        });
      }
    } catch {
      addToast({ type: 'error', title: 'Failed to trigger optimization' });
    }
  };

  const handleAcknowledge = (id: string) => {
    acknowledgeMutation.mutate(id, {
      onError: () => addToast({ type: 'error', title: 'Failed to acknowledge entry' }),
    });
  };

  const handleUnban = (address: string) => {
    unbanMutation.mutate(address);
  };

  const maxMarketsCap = dynamicTunerStatus?.opportunity_selection?.max_markets_cap ?? 0;
  const explorationRatio = useMemo(() => {
    if (!opportunitySettings || maxMarketsCap <= 0) {
      return 0;
    }
    return opportunitySettings.exploration_slots / maxMarketsCap;
  }, [opportunitySettings, maxMarketsCap]);

  const getActionIcon = (action: string) => {
    switch (action) {
      case 'probation_start':
        return <TrendingUp className="h-4 w-4 text-blue-500" />;
      case 'probation_graduate':
        return <CheckCircle className="h-4 w-4 text-green-500" />;
      case 'emergency_demote':
        return <AlertTriangle className="h-4 w-4 text-red-500" />;
      case 'grace_period_demote':
        return <TrendingDown className="h-4 w-4 text-orange-500" />;
      case 'pin':
        return <Shield className="h-4 w-4 text-purple-500" />;
      case 'ban':
        return <Ban className="h-4 w-4 text-red-500" />;
      default:
        return <Info className="h-4 w-4 text-gray-500" />;
    }
  };

  const getActionBadge = (action: string) => {
    const colors: Record<string, string> = {
      probation_start: 'bg-blue-500/10 text-blue-500',
      probation_graduate: 'bg-green-500/10 text-green-500',
      probation_fail: 'bg-red-500/10 text-red-500',
      emergency_demote: 'bg-red-500/10 text-red-500',
      grace_period_start: 'bg-yellow-500/10 text-yellow-500',
      grace_period_demote: 'bg-orange-500/10 text-orange-500',
      pin: 'bg-purple-500/10 text-purple-500',
      unpin: 'bg-gray-500/10 text-gray-500',
      ban: 'bg-red-500/10 text-red-500',
      unban: 'bg-green-500/10 text-green-500',
    };
    return colors[action] || 'bg-gray-500/10 text-gray-500';
  };

  return (
    <Card className="border-border/50 bg-card/50">
      <CardHeader className="pb-3">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <CardTitle className="flex items-center gap-2 text-lg">
            <Bot className="h-5 w-5 text-primary" />
            Automation
          </CardTitle>
          <Button
            variant="outline"
            size="sm"
            onClick={handleTriggerOptimization}
            disabled={triggerOptimizationMutation.isPending || !hasWorkspace}
          >
            {triggerOptimizationMutation.isPending ? (
              <RotateCcw className="h-4 w-4 animate-spin" />
            ) : (
              <Play className="h-4 w-4" />
            )}
            <span className="ml-2">Run Now</span>
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        <Tabs defaultValue="settings">
          <TabsList className="grid w-full grid-cols-2 sm:grid-cols-4">
            <TabsTrigger value="settings" className="text-xs">
              <Settings className="mr-1 h-3 w-3" />
              Settings
            </TabsTrigger>
            <TabsTrigger value="tuner" className="text-xs">
              <Gauge className="mr-1 h-3 w-3" />
              Tuner
            </TabsTrigger>
            <TabsTrigger value="history" className="text-xs">
              <History className="mr-1 h-3 w-3" />
              History
            </TabsTrigger>
            <TabsTrigger value="bans" className="text-xs">
              <Ban className="mr-1 h-3 w-3" />
              Bans ({bans.length})
            </TabsTrigger>
          </TabsList>

          <TabsContent value="settings" className="mt-4 space-y-4">
            {!hasWorkspace || isStatusLoading || !settings ? (
              <div className="rounded-lg border bg-background/50 p-4 text-sm text-muted-foreground">
                Loading automation settings...
              </div>
            ) : (
              <>
                <div className="rounded-lg border bg-background/50 p-4">
                  <div className="flex items-center justify-between">
                    <div className="space-y-1">
                      <p className="text-sm font-medium">Auto-Optimization</p>
                      <p className="text-xs text-muted-foreground">
                        Enable scheduled optimization and manual trigger runs.
                      </p>
                    </div>
                    <Switch
                      checked={settings.auto_optimize_enabled}
                      onCheckedChange={(checked) => updateSettings({ auto_optimize_enabled: checked })}
                    />
                  </div>
                  <div className="mt-3 flex flex-wrap gap-2 text-xs">
                    <Badge variant="outline">
                      Last run:{' '}
                      {optimizerStatus?.last_run_at
                        ? formatDistanceToNow(new Date(optimizerStatus.last_run_at), {
                            addSuffix: true,
                          })
                        : 'never'}
                    </Badge>
                    <Badge variant="outline">
                      Next run:{' '}
                      {optimizerStatus?.next_run_at
                        ? formatDistanceToNow(new Date(optimizerStatus.next_run_at), {
                            addSuffix: true,
                          })
                        : 'n/a'}
                    </Badge>
                  </div>
                </div>

                <div className="space-y-2">
                  <Label className="text-xs text-muted-foreground">Risk Appetite</Label>
                  <div className="grid gap-2 md:grid-cols-3">
                    {(Object.keys(RISK_PRESETS) as RiskPreset[]).map((preset) => (
                      <Button
                        key={preset}
                        type="button"
                        variant="outline"
                        size="sm"
                        className="h-auto py-2 text-left"
                        onClick={() => applyPreset(preset)}
                      >
                        <span className="block text-sm font-medium">{RISK_PRESETS[preset].label}</span>
                      </Button>
                    ))}
                  </div>
                </div>

                <div className="grid gap-4 md:grid-cols-2">
                  <div className="space-y-1.5">
                    <Label htmlFor="optimization-interval">Run Interval (hours)</Label>
                    <Input
                      id="optimization-interval"
                      type="number"
                      min={1}
                      max={168}
                      value={settings.optimization_interval_hours}
                      onChange={(e) => {
                        const next = Number(e.target.value);
                        if (!Number.isFinite(next)) {
                          return;
                        }
                        updateSettings({ optimization_interval_hours: clamp(Math.round(next), 1, 168) });
                      }}
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="min-trades">Min Trades (30d)</Label>
                    <Input
                      id="min-trades"
                      type="number"
                      min={0}
                      value={settings.min_trades_30d}
                      onChange={(e) => {
                        const next = Number(e.target.value);
                        if (!Number.isFinite(next)) {
                          return;
                        }
                        updateSettings({ min_trades_30d: Math.max(0, Math.round(next)) });
                      }}
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="min-roi">Min ROI (30d %)</Label>
                    <Input
                      id="min-roi"
                      type="number"
                      min={0}
                      step={0.1}
                      value={settings.min_roi_30d}
                      onChange={(e) => {
                        const next = Number(e.target.value);
                        if (!Number.isFinite(next)) {
                          return;
                        }
                        updateSettings({ min_roi_30d: Math.max(0, next) });
                      }}
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="min-win-rate">Min Win Rate (%)</Label>
                    <Input
                      id="min-win-rate"
                      type="number"
                      min={0}
                      max={100}
                      step={0.1}
                      value={settings.min_win_rate}
                      onChange={(e) => {
                        const next = Number(e.target.value);
                        if (!Number.isFinite(next)) {
                          return;
                        }
                        updateSettings({ min_win_rate: clamp(next, 0, 100) });
                      }}
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="min-sharpe">Min Sharpe</Label>
                    <Input
                      id="min-sharpe"
                      type="number"
                      min={0}
                      step={0.1}
                      value={settings.min_sharpe}
                      onChange={(e) => {
                        const next = Number(e.target.value);
                        if (!Number.isFinite(next)) {
                          return;
                        }
                        updateSettings({ min_sharpe: Math.max(0, next) });
                      }}
                    />
                  </div>
                </div>

                <div className="flex justify-stretch sm:justify-end">
                  <Button
                    size="sm"
                    className="w-full sm:w-auto"
                    onClick={handleSaveSettings}
                    disabled={!hasUnsavedChanges || saveSettingsMutation.isPending}
                  >
                    {saveSettingsMutation.isPending ? (
                      <RotateCcw className="mr-2 h-4 w-4 animate-spin" />
                    ) : (
                      <Save className="mr-2 h-4 w-4" />
                    )}
                    Save Settings
                  </Button>
                </div>
              </>
            )}
          </TabsContent>

          <TabsContent value="tuner" className="mt-4 space-y-3">
            {!hasWorkspace || isTunerLoading || !dynamicTunerStatus ? (
              <div className="rounded-lg border bg-background/50 p-4 text-sm text-muted-foreground">
                Loading dynamic tuner status...
              </div>
            ) : (
              <>
                <div className="rounded-lg border bg-background/50 p-4">
                  <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
                    <p className="text-sm font-medium">Opportunity Selection</p>
                    <Badge variant="outline">
                      Profile: {dynamicTunerStatus.opportunity_selection.aggressiveness}
                    </Badge>
                  </div>
                  {!opportunitySettings ? (
                    <p className="text-sm text-muted-foreground">Loading opportunity settings...</p>
                  ) : (
                    <>
                      <div className="grid gap-2 md:grid-cols-3">
                        {(Object.keys(OPPORTUNITY_PRESETS) as OpportunityAggressiveness[]).map(
                          (aggressiveness) => (
                            <Button
                              key={aggressiveness}
                              type="button"
                              size="sm"
                              variant={
                                opportunitySettings.aggressiveness === aggressiveness
                                  ? 'default'
                                  : 'outline'
                              }
                              className="h-auto justify-start py-2 text-left"
                              onClick={() => updateOpportunitySettings({ aggressiveness })}
                            >
                              <span className="block text-sm font-medium">
                                {OPPORTUNITY_PRESETS[aggressiveness].label}
                              </span>
                            </Button>
                          )
                        )}
                      </div>

                      <div className="mt-3 space-y-1.5">
                        <Label htmlFor="exploration-slots">Exploration Slots</Label>
                        <Input
                          id="exploration-slots"
                          type="number"
                          min={1}
                          max={500}
                          value={opportunitySettings.exploration_slots}
                          onChange={(event) => {
                            const next = Number(event.target.value);
                            if (!Number.isFinite(next)) {
                              return;
                            }
                            updateOpportunitySettings({
                              exploration_slots: clamp(Math.round(next), 1, 500),
                            });
                          }}
                        />
                        <p className="text-xs text-muted-foreground">
                          {dynamicTunerStatus.opportunity_selection.recommendation}
                        </p>
                      </div>

                      <div className="mt-3 grid gap-2 text-xs md:grid-cols-2">
                        <Badge variant="outline" className="justify-start">
                          Max monitored markets: {dynamicTunerStatus.opportunity_selection.max_markets_cap}
                        </Badge>
                        <Badge variant="outline" className="justify-start">
                          Exploration ratio: {(explorationRatio * 100).toFixed(0)}%
                        </Badge>
                      </div>

                      {maxMarketsCap > 0 &&
                        opportunitySettings.exploration_slots >= maxMarketsCap && (
                          <p className="mt-2 text-xs text-red-500">
                            Exploration slots must stay below monitored market capacity ({maxMarketsCap}).
                          </p>
                        )}
                      {maxMarketsCap > 0 && explorationRatio > 0.6 && explorationRatio < 1 && (
                        <p className="mt-2 text-xs text-yellow-600">
                          High discovery mode: this can increase opportunity churn and resubscribe frequency.
                        </p>
                      )}

                      <div className="mt-3 flex flex-col gap-2 sm:flex-row sm:justify-end">
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          onClick={applyOpportunityPresetDefaults}
                        >
                          Use Recommended Defaults
                        </Button>
                        <Button
                          type="button"
                          size="sm"
                          onClick={handleSaveOpportunitySettings}
                          disabled={
                            !hasOpportunityUnsavedChanges ||
                            saveOpportunitySelectionMutation.isPending ||
                            (maxMarketsCap > 0 &&
                              opportunitySettings.exploration_slots >= maxMarketsCap)
                          }
                        >
                          {saveOpportunitySelectionMutation.isPending ? (
                            <RotateCcw className="mr-2 h-4 w-4 animate-spin" />
                          ) : (
                            <Save className="mr-2 h-4 w-4" />
                          )}
                          Save Opportunity Settings
                        </Button>
                      </div>
                    </>
                  )}
                </div>

                <div className="rounded-lg border bg-background/50 p-4">
                  <p className="mb-2 text-sm font-medium">Scanner Runtime</p>
                  <div className="grid gap-2 text-sm md:grid-cols-3">
                    <div className="rounded border border-border/50 px-3 py-2">
                      <p className="text-xs text-muted-foreground">Monitored</p>
                      <p className="font-semibold">{dynamicTunerStatus.scanner_status.monitored_markets}</p>
                    </div>
                    <div className="rounded border border-border/50 px-3 py-2">
                      <p className="text-xs text-muted-foreground">Core</p>
                      <p className="font-semibold">{dynamicTunerStatus.scanner_status.core_markets}</p>
                    </div>
                    <div className="rounded border border-border/50 px-3 py-2">
                      <p className="text-xs text-muted-foreground">Exploration</p>
                      <p className="font-semibold">{dynamicTunerStatus.scanner_status.exploration_markets}</p>
                    </div>
                  </div>
                  <div className="mt-3 grid gap-2 text-xs text-muted-foreground md:grid-cols-2">
                    <div>
                      Last re-rank:{' '}
                      {dynamicTunerStatus.scanner_status.last_rerank_at
                        ? formatDistanceToNow(new Date(dynamicTunerStatus.scanner_status.last_rerank_at), {
                            addSuffix: true,
                          })
                        : 'not recorded'}
                    </div>
                    <div>
                      Last resubscribe:{' '}
                      {dynamicTunerStatus.scanner_status.last_resubscribe_at
                        ? formatDistanceToNow(
                            new Date(dynamicTunerStatus.scanner_status.last_resubscribe_at),
                            {
                              addSuffix: true,
                            }
                          )
                        : 'not recorded'}
                    </div>
                  </div>
                </div>

                <div className="rounded-lg border bg-background/50 p-4">
                  <div className="flex flex-wrap items-center gap-2">
                    <Badge variant={dynamicTunerStatus.enabled ? 'default' : 'secondary'}>
                      {dynamicTunerStatus.enabled ? 'Enabled' : 'Disabled'}
                    </Badge>
                    <Badge variant="outline">
                      {dynamicTunerStatus.mode === 'apply' ? 'Apply mode' : 'Shadow mode'}
                    </Badge>
                    <Badge variant="outline">Regime: {dynamicTunerStatus.current_regime}</Badge>
                    <Badge variant={dynamicTunerStatus.frozen ? 'destructive' : 'secondary'}>
                      {dynamicTunerStatus.frozen ? 'Frozen' : 'Active'}
                    </Badge>
                  </div>
                  <div className="mt-3 grid gap-2 text-xs text-muted-foreground md:grid-cols-2">
                    <div>
                      Last run:{' '}
                      {dynamicTunerStatus.last_run_at
                        ? formatDistanceToNow(new Date(dynamicTunerStatus.last_run_at), {
                            addSuffix: true,
                          })
                        : 'not recorded'}
                    </div>
                    <div>
                      Last change:{' '}
                      {dynamicTunerStatus.last_change_at
                        ? formatDistanceToNow(new Date(dynamicTunerStatus.last_change_at), {
                            addSuffix: true,
                          })
                        : 'none'}
                    </div>
                  </div>
                  {dynamicTunerStatus.freeze_reason && (
                    <p className="mt-2 text-xs text-red-500">{dynamicTunerStatus.freeze_reason}</p>
                  )}
                </div>

                <div className="rounded-lg border bg-background/50 p-4">
                  <p className="mb-2 text-sm font-medium">Why Markets Were Selected</p>
                  {dynamicTunerStatus.scanner_status.selected_markets.length === 0 ? (
                    <p className="text-sm text-muted-foreground">
                      No live selection snapshot available yet.
                    </p>
                  ) : (
                    <div className="space-y-2">
                      {dynamicTunerStatus.scanner_status.selected_markets.map((market) => (
                        <div
                          key={`${market.market_id}-${market.tier}`}
                          className="rounded border border-border/50 px-3 py-2 text-xs"
                        >
                          <div className="mb-1 flex items-center justify-between gap-2">
                            <span className="font-mono break-all">{market.market_id}</span>
                            <Badge variant={market.tier === 'exploration' ? 'secondary' : 'outline'}>
                              {market.tier}
                            </Badge>
                          </div>
                          <div className="grid gap-1 text-muted-foreground md:grid-cols-3">
                            <span>Total: {market.total_score.toFixed(2)}</span>
                            <span>Baseline: {market.baseline_score.toFixed(2)}</span>
                            <span>Opportunity: {market.opportunity_score.toFixed(2)}</span>
                            <span>Hit-rate: {market.hit_rate_score.toFixed(2)}</span>
                            <span>Freshness: {market.freshness_score.toFixed(2)}</span>
                            <span>Sticky: {market.sticky_score.toFixed(2)}</span>
                            {market.novelty_score != null && (
                              <span>Novelty: {market.novelty_score.toFixed(2)}</span>
                            )}
                            {market.rotation_score != null && (
                              <span>Rotation: {market.rotation_score.toFixed(2)}</span>
                            )}
                            {market.upside_score != null && (
                              <span>Upside: {market.upside_score.toFixed(2)}</span>
                            )}
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </div>

                <div className="rounded-lg border bg-background/50 p-4">
                  <p className="mb-2 text-sm font-medium">Current Dynamic Config</p>
                  <div className="space-y-2">
                    {dynamicTunerStatus.dynamic_config.map((entry) => (
                      <div
                        key={entry.key}
                        className="flex flex-col gap-1 rounded border border-border/50 px-3 py-2 text-sm sm:flex-row sm:items-center sm:justify-between"
                      >
                        <span className="font-mono text-xs break-all">{entry.key}</span>
                        <span className="font-medium tabular-nums">{entry.current_value.toFixed(4)}</span>
                      </div>
                    ))}
                  </div>
                </div>
              </>
            )}
          </TabsContent>

          <TabsContent value="history" className="mt-4">
            <div className="max-h-64 space-y-2 overflow-y-auto">
              {history.length === 0 ? (
                <div className="py-8 text-center text-sm text-muted-foreground">
                  No automation history yet
                </div>
              ) : (
                history.map((entry) => (
                  <div
                    key={entry.id}
                    className={`flex items-start gap-3 rounded-lg border bg-background/50 p-3 ${
                      !entry.acknowledged ? 'border-primary/20' : 'border-border/50'
                    }`}
                  >
                    <div className="mt-0.5">{getActionIcon(entry.action)}</div>
                    <div className="min-w-0 flex-1">
                      <div className="mb-1 flex items-center gap-2">
                        <Badge className={`text-xs ${getActionBadge(entry.action)}`}>
                          {entry.action.replace(/_/g, ' ')}
                        </Badge>
                        <span className="text-xs text-muted-foreground">
                          {formatDistanceToNow(new Date(entry.created_at), { addSuffix: true })}
                        </span>
                      </div>
                      <p className="truncate text-sm">{entry.reason}</p>
                      {(entry.wallet_in || entry.wallet_out) && (
                        <div className="mt-1 font-mono text-xs text-muted-foreground">
                          {entry.wallet_in && (
                            <span className="text-green-500">+{entry.wallet_in.slice(0, 8)}...</span>
                          )}
                          {entry.wallet_in && entry.wallet_out && ' / '}
                          {entry.wallet_out && (
                            <span className="text-red-500">-{entry.wallet_out.slice(0, 8)}...</span>
                          )}
                        </div>
                      )}
                    </div>
                    {!entry.acknowledged && (
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => handleAcknowledge(entry.id)}
                        disabled={acknowledgeMutation.isPending}
                      >
                        <CheckCircle className="h-4 w-4" />
                      </Button>
                    )}
                  </div>
                ))
              )}
            </div>
          </TabsContent>

          <TabsContent value="bans" className="mt-4">
            <div className="max-h-64 space-y-2 overflow-y-auto">
              {bans.length === 0 ? (
                <div className="py-8 text-center text-sm text-muted-foreground">No banned wallets</div>
              ) : (
                bans.map((ban: WalletBan) => (
                  <div
                    key={ban.id}
                    className="flex flex-col gap-2 rounded-lg border border-border/50 bg-background/50 p-3 sm:flex-row sm:items-center sm:justify-between"
                  >
                    <div className="flex min-w-0 items-center gap-3">
                      <Ban className="h-4 w-4 text-red-500" />
                      <div className="min-w-0">
                        <p className="text-sm font-mono">
                          {ban.wallet_address.slice(0, 10)}...{ban.wallet_address.slice(-8)}
                        </p>
                        {ban.reason && <p className="text-xs text-muted-foreground">{ban.reason}</p>}
                        {ban.expires_at && (
                          <p className="text-xs text-muted-foreground">
                            Expires {formatDistanceToNow(new Date(ban.expires_at), { addSuffix: true })}
                          </p>
                        )}
                      </div>
                    </div>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="self-end sm:self-auto"
                      onClick={() => handleUnban(ban.wallet_address)}
                      disabled={unbanMutation.isPending}
                    >
                      <XCircle className="h-4 w-4" />
                    </Button>
                  </div>
                ))
              )}
            </div>
          </TabsContent>
        </Tabs>
      </CardContent>
    </Card>
  );
}
