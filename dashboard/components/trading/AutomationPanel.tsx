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
import { ratioOrPercentToPercent, formatDynamicKey, formatDynamicConfigValue } from '@/lib/utils';
import { RISK_PRESETS } from '@/lib/riskPresets';
import type { RiskPreset } from '@/lib/riskPresets';
import { TunerTimeline } from '@/components/analytics/TunerTimeline';
import { useDynamicConfigHistoryQuery } from '@/hooks/queries/useHistoryQuery';
import { useNotificationStore } from '@/stores/notification-store';
import { useToastStore } from '@/stores/toast-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import {
  useAcknowledgeRotationMutation,
  useOptimizerStatusQuery,
  useRotationHistoryQuery,
  useTriggerOptimizationMutation,
} from '@/hooks/queries/useOptimizerQuery';
import { api } from '@/lib/api';
import {
  AlertTriangle,
  ArrowLeftRight,
  Ban,
  Bot,
  CheckCircle,
  Clock,
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

type OpportunityAggressiveness = 'stable' | 'balanced' | 'discovery';

const COPY_TRADING_KEYS = new Set([
  'COPY_MIN_TRADE_VALUE',
  'COPY_MAX_SLIPPAGE_PCT',
  'COPY_MAX_LATENCY_SECS',
  'COPY_DAILY_CAPITAL_LIMIT',
  'COPY_MAX_OPEN_POSITIONS',
  'COPY_STOP_LOSS_PCT',
  'COPY_TAKE_PROFIT_PCT',
  'COPY_MAX_HOLD_HOURS',
  'COPY_TOTAL_CAPITAL',
  'COPY_NEAR_RESOLUTION_MARGIN',
]);

const ARB_EXECUTOR_KEYS = new Set([
  'ARB_POSITION_SIZE',
  'ARB_MIN_NET_PROFIT',
  'ARB_MIN_BOOK_DEPTH',
  'ARB_MAX_SIGNAL_AGE_SECS',
]);

interface CopyTradingDraft {
  min_trade_value: number;
  max_slippage_pct: number;
  max_latency_secs: number;
  daily_capital_limit: number;
  max_open_positions: number;
  stop_loss_pct: number;
  take_profit_pct: number;
  max_hold_hours: number;
  total_capital: number;
  near_resolution_margin: number;
}

interface ArbExecutorDraft {
  position_size: number;
  min_net_profit: number;
  min_book_depth: number;
  max_signal_age_secs: number;
}

interface OpportunitySelectionDraft {
  aggressiveness: OpportunityAggressiveness;
  exploration_slots: number;
}

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
  const { currentWorkspace } = useWorkspaceStore();
  const isOwner = currentWorkspace?.my_role === 'owner';
  const hasWorkspace = Boolean(workspaceId);

  const { data: optimizerStatus, isLoading: isStatusLoading } = useOptimizerStatusQuery(
    hasWorkspace ? workspaceId : undefined
  );
  const [historyLimit, setHistoryLimit] = useState(20);
  const { data: history = [] } = useRotationHistoryQuery({
    workspaceId: hasWorkspace ? workspaceId : undefined,
    limit: historyLimit,
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
  const { data: dynamicConfigHistory = [] } = useDynamicConfigHistoryQuery({
    workspaceId: hasWorkspace ? workspaceId : undefined,
    limit: 50,
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

  // Copy trading config - baseline/draft pattern
  const [copyTradingDraft, setCopyTradingDraft] = useState<CopyTradingDraft | null>(null);

  const baselineCopyTrading = useMemo<CopyTradingDraft | null>(() => {
    if (!dynamicTunerStatus?.dynamic_config) return null;
    const configs = dynamicTunerStatus.dynamic_config;
    const findVal = (key: string) => configs.find((c) => c.key === key)?.current_value;
    // Need at least one copy trading key present to show the section
    const hasCopyKeys = configs.some((c) => COPY_TRADING_KEYS.has(c.key));
    if (!hasCopyKeys) return null;
    return {
      min_trade_value: findVal('COPY_MIN_TRADE_VALUE') ?? 2,
      max_slippage_pct: findVal('COPY_MAX_SLIPPAGE_PCT') ?? 0.01,
      max_latency_secs: findVal('COPY_MAX_LATENCY_SECS') ?? 120,
      daily_capital_limit: findVal('COPY_DAILY_CAPITAL_LIMIT') ?? 5000,
      max_open_positions: findVal('COPY_MAX_OPEN_POSITIONS') ?? 15,
      stop_loss_pct: findVal('COPY_STOP_LOSS_PCT') ?? 0.15,
      take_profit_pct: findVal('COPY_TAKE_PROFIT_PCT') ?? 0.25,
      max_hold_hours: findVal('COPY_MAX_HOLD_HOURS') ?? 72,
      total_capital: findVal('COPY_TOTAL_CAPITAL') ?? 10000,
      near_resolution_margin: findVal('COPY_NEAR_RESOLUTION_MARGIN') ?? 0.03,
    };
  }, [dynamicTunerStatus]);

  useEffect(() => {
    if (!baselineCopyTrading) return;
    setCopyTradingDraft((current) => current ?? baselineCopyTrading);
  }, [baselineCopyTrading]);

  // Reset copy trading draft on workspace change
  useEffect(() => {
    setCopyTradingDraft(null);
  }, [workspaceId]);

  const hasCopyTradingChanges = useMemo(() => {
    if (!copyTradingDraft || !baselineCopyTrading) return false;
    return JSON.stringify(copyTradingDraft) !== JSON.stringify(baselineCopyTrading);
  }, [copyTradingDraft, baselineCopyTrading]);

  const saveCopyTradingMutation = useMutation({
    mutationFn: () => {
      if (!copyTradingDraft) throw new Error('No draft');
      return api.updateCopyTradingConfig(workspaceId, {
        min_trade_value: copyTradingDraft.min_trade_value,
        max_slippage_pct: copyTradingDraft.max_slippage_pct,
        max_latency_secs: Math.round(copyTradingDraft.max_latency_secs),
        daily_capital_limit: copyTradingDraft.daily_capital_limit,
        max_open_positions: Math.round(copyTradingDraft.max_open_positions),
        stop_loss_pct: copyTradingDraft.stop_loss_pct,
        take_profit_pct: copyTradingDraft.take_profit_pct,
        max_hold_hours: Math.round(copyTradingDraft.max_hold_hours),
        total_capital: copyTradingDraft.total_capital,
        near_resolution_margin: copyTradingDraft.near_resolution_margin,
      });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['dynamic-tuner-status'] });
      addToast({ type: 'success', title: 'Copy trading config saved' });
      // Reset baseline so draft re-syncs on next fetch
      setCopyTradingDraft(null);
    },
    onError: () => {
      addToast({ type: 'error', title: 'Failed to save copy trading config' });
    },
  });

  // Arb executor config - baseline/draft pattern
  const [arbExecutorDraft, setArbExecutorDraft] = useState<ArbExecutorDraft | null>(null);

  const baselineArbExecutor = useMemo<ArbExecutorDraft | null>(() => {
    if (!dynamicTunerStatus?.dynamic_config) return null;
    const configs = dynamicTunerStatus.dynamic_config;
    const findVal = (key: string) => configs.find((c) => c.key === key)?.current_value;
    const hasArbKeys = configs.some((c) => ARB_EXECUTOR_KEYS.has(c.key));
    if (!hasArbKeys) return null;
    return {
      position_size: findVal('ARB_POSITION_SIZE') ?? 50,
      min_net_profit: findVal('ARB_MIN_NET_PROFIT') ?? 0.001,
      min_book_depth: findVal('ARB_MIN_BOOK_DEPTH') ?? 100,
      max_signal_age_secs: findVal('ARB_MAX_SIGNAL_AGE_SECS') ?? 30,
    };
  }, [dynamicTunerStatus]);

  useEffect(() => {
    if (!baselineArbExecutor) return;
    setArbExecutorDraft((current) => current ?? baselineArbExecutor);
  }, [baselineArbExecutor]);

  useEffect(() => {
    setArbExecutorDraft(null);
  }, [workspaceId]);

  const hasArbExecutorChanges = useMemo(() => {
    if (!arbExecutorDraft || !baselineArbExecutor) return false;
    return JSON.stringify(arbExecutorDraft) !== JSON.stringify(baselineArbExecutor);
  }, [arbExecutorDraft, baselineArbExecutor]);

  const saveArbExecutorMutation = useMutation({
    mutationFn: () => {
      if (!arbExecutorDraft) throw new Error('No draft');
      return api.updateArbExecutorConfig(workspaceId, {
        position_size: arbExecutorDraft.position_size,
        min_net_profit: arbExecutorDraft.min_net_profit,
        min_book_depth: arbExecutorDraft.min_book_depth,
        max_signal_age_secs: arbExecutorDraft.max_signal_age_secs,
      });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['dynamic-tuner-status'] });
      addToast({ type: 'success', title: 'Arb executor config saved' });
      setArbExecutorDraft(null);
    },
    onError: () => {
      addToast({ type: 'error', title: 'Failed to save arb executor config' });
    },
  });

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

  const getActionIcon = (action: string, evidence?: Record<string, unknown>) => {
    // Inactivity demotions are logged as emergency_demote with evidence.trigger = 'Inactivity'
    if (action === 'emergency_demote' && evidence?.trigger === 'Inactivity') {
      return <Clock className="h-4 w-4 text-amber-500" />;
    }
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
      case 'auto_swap':
        return <ArrowLeftRight className="h-4 w-4 text-blue-500" />;
      case 'undo':
        return <RotateCcw className="h-4 w-4 text-gray-500" />;
      default:
        return <Info className="h-4 w-4 text-gray-500" />;
    }
  };

  const getActionBadge = (action: string, evidence?: Record<string, unknown>) => {
    // Inactivity demotions get amber styling
    if (action === 'emergency_demote' && evidence?.trigger === 'Inactivity') {
      return 'bg-amber-500/10 text-amber-500';
    }
    const colors: Record<string, string> = {
      probation_start: 'bg-blue-500/10 text-blue-500',
      probation_graduate: 'bg-green-500/10 text-green-500',
      probation_fail: 'bg-red-500/10 text-red-500',
      emergency_demote: 'bg-red-500/10 text-red-500',
      grace_period_start: 'bg-yellow-500/10 text-yellow-500',
      grace_period_demote: 'bg-orange-500/10 text-orange-500',
      auto_swap: 'bg-blue-500/10 text-blue-500',
      pin: 'bg-purple-500/10 text-purple-500',
      unpin: 'bg-gray-500/10 text-gray-500',
      ban: 'bg-red-500/10 text-red-500',
      unban: 'bg-green-500/10 text-green-500',
      undo: 'bg-gray-500/10 text-gray-500',
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
          <TabsList className="grid w-full grid-cols-3 sm:grid-cols-5">
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
            <TabsTrigger value="tuner-log" className="text-xs">
              <Clock className="mr-1 h-3 w-3" />
              Tuner Log
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
                  {!isOwner ? (
                    <p className="text-sm text-muted-foreground">
                      Opportunity selection can only be changed by the workspace owner.
                    </p>
                  ) : !opportunitySettings ? (
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

                {/* Editable Copy Trading Thresholds */}
                {copyTradingDraft && (
                  <div className="rounded-lg border bg-background/50 p-4">
                    <p className="mb-3 text-sm font-medium">Copy Trading Thresholds</p>
                    <div className="space-y-4">
                      {/* Min Trade Value */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Min Copy Trade Value</p>
                          <p className="text-xs text-muted-foreground">Trades below this USD amount are skipped</p>
                        </div>
                        <div className="flex items-center gap-1">
                          <span className="text-sm text-muted-foreground">$</span>
                          <Input
                            type="number"
                            min={0.5}
                            max={50}
                            step={0.5}
                            value={copyTradingDraft.min_trade_value}
                            onChange={(e) => {
                              const v = Number(e.target.value);
                              if (!Number.isFinite(v)) return;
                              setCopyTradingDraft((d) =>
                                d ? { ...d, min_trade_value: Math.max(0.5, Math.min(50, v)) } : d
                              );
                            }}
                            className="w-24 text-right"
                          />
                        </div>
                      </div>

                      {/* Max Slippage */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Max Copy Slippage</p>
                          <p className="text-xs text-muted-foreground">Maximum acceptable price slippage</p>
                        </div>
                        <div className="flex items-center gap-1">
                          <Input
                            type="number"
                            min={0.025}
                            max={15}
                            step={0.1}
                            value={Math.round(copyTradingDraft.max_slippage_pct * 10000) / 100}
                            onChange={(e) => {
                              const pctVal = Number(e.target.value);
                              if (!Number.isFinite(pctVal)) return;
                              const ratio = Math.max(0.0025, Math.min(0.15, pctVal / 100));
                              setCopyTradingDraft((d) =>
                                d ? { ...d, max_slippage_pct: ratio } : d
                              );
                            }}
                            className="w-24 text-right"
                          />
                          <span className="text-sm text-muted-foreground">%</span>
                        </div>
                      </div>

                      {/* Max Trade Age (latency) */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Max Copy Trade Age</p>
                          <p className="text-xs text-muted-foreground">Trades older than this are considered stale</p>
                        </div>
                        <div className="flex items-center gap-1">
                          <Input
                            type="number"
                            min={1}
                            max={5}
                            step={0.5}
                            value={Math.round((copyTradingDraft.max_latency_secs / 60) * 10) / 10}
                            onChange={(e) => {
                              const mins = Number(e.target.value);
                              if (!Number.isFinite(mins)) return;
                              const secs = Math.max(60, Math.min(300, mins * 60));
                              setCopyTradingDraft((d) =>
                                d ? { ...d, max_latency_secs: secs } : d
                              );
                            }}
                            className="w-24 text-right"
                          />
                          <span className="text-sm text-muted-foreground">min</span>
                        </div>
                      </div>

                      <div className="my-3 border-t border-border/50" />
                      <p className="text-xs font-medium text-muted-foreground uppercase tracking-wider">Risk & Position Limits</p>

                      {/* Daily Capital Limit */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Daily Capital Limit</p>
                          <p className="text-xs text-muted-foreground">Max USD deployed per day across all copy trades</p>
                        </div>
                        <div className="flex items-center gap-1">
                          <span className="text-sm text-muted-foreground">$</span>
                          <Input
                            type="number"
                            min={100}
                            max={50000}
                            step={100}
                            value={copyTradingDraft.daily_capital_limit}
                            onChange={(e) => {
                              const v = Number(e.target.value);
                              if (!Number.isFinite(v)) return;
                              setCopyTradingDraft((d) =>
                                d ? { ...d, daily_capital_limit: clamp(v, 100, 50000) } : d
                              );
                            }}
                            className="w-28 text-right"
                          />
                        </div>
                      </div>

                      {/* Max Open Positions */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Max Open Positions</p>
                          <p className="text-xs text-muted-foreground">Maximum concurrent copy-traded positions</p>
                        </div>
                        <Input
                          type="number"
                          min={1}
                          max={50}
                          step={1}
                          value={Math.round(copyTradingDraft.max_open_positions)}
                          onChange={(e) => {
                            const v = Number(e.target.value);
                            if (!Number.isFinite(v)) return;
                            setCopyTradingDraft((d) =>
                              d ? { ...d, max_open_positions: clamp(Math.round(v), 1, 50) } : d
                            );
                          }}
                          className="w-24 text-right"
                        />
                      </div>

                      {/* Stop-Loss % */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Stop-Loss</p>
                          <p className="text-xs text-muted-foreground">Auto-exit when loss reaches this percentage</p>
                        </div>
                        <div className="flex items-center gap-1">
                          <Input
                            type="number"
                            min={5}
                            max={50}
                            step={1}
                            value={Math.round(copyTradingDraft.stop_loss_pct * 1000) / 10}
                            onChange={(e) => {
                              const pctVal = Number(e.target.value);
                              if (!Number.isFinite(pctVal)) return;
                              const ratio = clamp(pctVal / 100, 0.05, 0.50);
                              setCopyTradingDraft((d) =>
                                d ? { ...d, stop_loss_pct: ratio } : d
                              );
                            }}
                            className="w-24 text-right"
                          />
                          <span className="text-sm text-muted-foreground">%</span>
                        </div>
                      </div>

                      {/* Take-Profit % */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Take-Profit</p>
                          <p className="text-xs text-muted-foreground">Auto-exit when profit reaches this percentage</p>
                        </div>
                        <div className="flex items-center gap-1">
                          <Input
                            type="number"
                            min={5}
                            max={100}
                            step={1}
                            value={Math.round(copyTradingDraft.take_profit_pct * 1000) / 10}
                            onChange={(e) => {
                              const pctVal = Number(e.target.value);
                              if (!Number.isFinite(pctVal)) return;
                              const ratio = clamp(pctVal / 100, 0.05, 1.00);
                              setCopyTradingDraft((d) =>
                                d ? { ...d, take_profit_pct: ratio } : d
                              );
                            }}
                            className="w-24 text-right"
                          />
                          <span className="text-sm text-muted-foreground">%</span>
                        </div>
                      </div>

                      {/* Max Hold Hours */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Max Hold Duration</p>
                          <p className="text-xs text-muted-foreground">Force-exit positions held longer than this</p>
                        </div>
                        <div className="flex items-center gap-1">
                          <Input
                            type="number"
                            min={1}
                            max={720}
                            step={1}
                            value={Math.round(copyTradingDraft.max_hold_hours)}
                            onChange={(e) => {
                              const v = Number(e.target.value);
                              if (!Number.isFinite(v)) return;
                              setCopyTradingDraft((d) =>
                                d ? { ...d, max_hold_hours: clamp(Math.round(v), 1, 720) } : d
                              );
                            }}
                            className="w-24 text-right"
                          />
                          <span className="text-sm text-muted-foreground">hrs</span>
                        </div>
                      </div>

                      {/* Total Copy Capital */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Total Copy Capital</p>
                          <p className="text-xs text-muted-foreground">Overall budget for copy trading positions</p>
                        </div>
                        <div className="flex items-center gap-1">
                          <span className="text-sm text-muted-foreground">$</span>
                          <Input
                            type="number"
                            min={100}
                            max={500000}
                            step={100}
                            value={copyTradingDraft.total_capital}
                            onChange={(e) => {
                              const v = Number(e.target.value);
                              if (!Number.isFinite(v)) return;
                              setCopyTradingDraft((d) =>
                                d ? { ...d, total_capital: clamp(v, 100, 500000) } : d
                              );
                            }}
                            className="w-28 text-right"
                          />
                        </div>
                      </div>

                      {/* Near-Resolution Margin */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Near-Resolution Margin</p>
                          <p className="text-xs text-muted-foreground">Skip markets within this margin of 0 or 1. Min 3% enforced by backend.</p>
                        </div>
                        <Input
                          type="number"
                          min={0.03}
                          max={0.25}
                          step={0.01}
                          value={copyTradingDraft.near_resolution_margin}
                          onChange={(e) => {
                            const v = Number(e.target.value);
                            if (!Number.isFinite(v)) return;
                            setCopyTradingDraft((d) =>
                              d ? { ...d, near_resolution_margin: clamp(v, 0.03, 0.25) } : d
                            );
                          }}
                          className="w-24 text-right"
                        />
                      </div>
                    </div>

                    <Button
                      className="mt-4 w-full"
                      disabled={!hasCopyTradingChanges || saveCopyTradingMutation.isPending}
                      onClick={() => saveCopyTradingMutation.mutate()}
                    >
                      {saveCopyTradingMutation.isPending ? (
                        <>
                          <RotateCcw className="mr-2 h-4 w-4 animate-spin" />
                          Saving...
                        </>
                      ) : hasCopyTradingChanges ? (
                        <>
                          <Save className="mr-2 h-4 w-4" />
                          Save Copy Trading Config
                        </>
                      ) : (
                        <>
                          <CheckCircle className="mr-2 h-4 w-4" />
                          Saved
                        </>
                      )}
                    </Button>
                  </div>
                )}

                {/* Editable Arb Executor Thresholds */}
                {arbExecutorDraft && (
                  <div className="rounded-lg border bg-background/50 p-4">
                    <p className="mb-3 text-sm font-medium">Arb Executor Thresholds</p>
                    <div className="space-y-4">
                      {/* Position Size */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Position Size</p>
                          <p className="text-xs text-muted-foreground">Default dollar amount per arb trade</p>
                        </div>
                        <div className="flex items-center gap-1">
                          <span className="text-sm text-muted-foreground">$</span>
                          <Input
                            type="number"
                            min={10}
                            max={500}
                            step={5}
                            value={arbExecutorDraft.position_size}
                            onChange={(e) => {
                              const v = Number(e.target.value);
                              if (!Number.isFinite(v)) return;
                              setArbExecutorDraft((d) =>
                                d ? { ...d, position_size: clamp(v, 10, 500) } : d
                              );
                            }}
                            className="w-24 text-right"
                          />
                        </div>
                      </div>

                      {/* Min Net Profit */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Min Net Profit</p>
                          <p className="text-xs text-muted-foreground">Skip signals with net profit below this threshold</p>
                        </div>
                        <Input
                          type="number"
                          min={0.0005}
                          max={0.05}
                          step={0.0005}
                          value={arbExecutorDraft.min_net_profit}
                          onChange={(e) => {
                            const v = Number(e.target.value);
                            if (!Number.isFinite(v)) return;
                            setArbExecutorDraft((d) =>
                              d ? { ...d, min_net_profit: clamp(v, 0.0005, 0.05) } : d
                            );
                          }}
                          className="w-24 text-right"
                        />
                      </div>

                      {/* Min Book Depth */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Min Book Depth</p>
                          <p className="text-xs text-muted-foreground">Require this much liquidity in the order book</p>
                        </div>
                        <div className="flex items-center gap-1">
                          <span className="text-sm text-muted-foreground">$</span>
                          <Input
                            type="number"
                            min={25}
                            max={1000}
                            step={25}
                            value={arbExecutorDraft.min_book_depth}
                            onChange={(e) => {
                              const v = Number(e.target.value);
                              if (!Number.isFinite(v)) return;
                              setArbExecutorDraft((d) =>
                                d ? { ...d, min_book_depth: clamp(v, 25, 1000) } : d
                              );
                            }}
                            className="w-24 text-right"
                          />
                        </div>
                      </div>

                      {/* Max Signal Age */}
                      <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                        <div>
                          <p className="text-sm font-medium">Max Signal Age</p>
                          <p className="text-xs text-muted-foreground">Discard arb signals older than this</p>
                        </div>
                        <div className="flex items-center gap-1">
                          <Input
                            type="number"
                            min={5}
                            max={300}
                            step={5}
                            value={arbExecutorDraft.max_signal_age_secs}
                            onChange={(e) => {
                              const v = Number(e.target.value);
                              if (!Number.isFinite(v)) return;
                              setArbExecutorDraft((d) =>
                                d ? { ...d, max_signal_age_secs: clamp(v, 5, 300) } : d
                              );
                            }}
                            className="w-24 text-right"
                          />
                          <span className="text-sm text-muted-foreground">s</span>
                        </div>
                      </div>
                    </div>

                    <Button
                      className="mt-4 w-full"
                      disabled={!hasArbExecutorChanges || saveArbExecutorMutation.isPending}
                      onClick={() => saveArbExecutorMutation.mutate()}
                    >
                      {saveArbExecutorMutation.isPending ? (
                        <>
                          <RotateCcw className="mr-2 h-4 w-4 animate-spin" />
                          Saving...
                        </>
                      ) : hasArbExecutorChanges ? (
                        <>
                          <Save className="mr-2 h-4 w-4" />
                          Save Arb Executor Config
                        </>
                      ) : (
                        <>
                          <CheckCircle className="mr-2 h-4 w-4" />
                          Saved
                        </>
                      )}
                    </Button>
                  </div>
                )}

                {/* Other Dynamic Config (read-only) */}
                {dynamicTunerStatus.dynamic_config.filter((e) => !COPY_TRADING_KEYS.has(e.key) && !ARB_EXECUTOR_KEYS.has(e.key)).length > 0 && (
                  <div className="rounded-lg border bg-background/50 p-4">
                    <p className="mb-2 text-sm font-medium">Other Dynamic Config</p>
                    <div className="space-y-2">
                      {dynamicTunerStatus.dynamic_config
                        .filter((entry) => !COPY_TRADING_KEYS.has(entry.key) && !ARB_EXECUTOR_KEYS.has(entry.key))
                        .map((entry) => (
                          <div
                            key={entry.key}
                            className="flex flex-col gap-1 rounded border border-border/50 px-3 py-2 text-sm sm:flex-row sm:items-center sm:justify-between"
                          >
                            <span className="font-medium text-xs">{formatDynamicKey(entry.key)}</span>
                            <span className="font-medium tabular-nums">{formatDynamicConfigValue(entry.key, entry.current_value)}</span>
                          </div>
                        ))}
                    </div>
                  </div>
                )}
              </>
            )}
          </TabsContent>

          <TabsContent value="history" className="mt-4">
            <div className="max-h-80 space-y-2 overflow-y-auto">
              {history.length === 0 ? (
                <div className="py-8 text-center text-sm text-muted-foreground">
                  No automation history yet
                </div>
              ) : (
                <>
                  {history.map((entry) => (
                    <div
                      key={entry.id}
                      className={`flex items-start gap-3 rounded-lg border bg-background/50 p-3 ${
                        !entry.acknowledged ? 'border-primary/20' : 'border-border/50'
                      }`}
                    >
                      <div className="mt-0.5">{getActionIcon(entry.action, entry.evidence)}</div>
                      <div className="min-w-0 flex-1">
                        <div className="mb-1 flex items-center gap-2">
                          <Badge className={`text-xs ${getActionBadge(entry.action, entry.evidence)}`}>
                            {entry.action === 'emergency_demote' && entry.evidence?.trigger === 'Inactivity'
                              ? 'inactivity demote'
                              : entry.action.replace(/_/g, ' ')}
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
                  ))}
                  {history.length >= historyLimit && (
                    <Button
                      variant="ghost"
                      size="sm"
                      className="w-full text-muted-foreground"
                      onClick={() => setHistoryLimit((prev) => prev + 20)}
                    >
                      Load more
                    </Button>
                  )}
                </>
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

          <TabsContent value="tuner-log" className="mt-4">
            <TunerTimeline history={dynamicConfigHistory} />
          </TabsContent>
        </Tabs>
      </CardContent>
    </Card>
  );
}
