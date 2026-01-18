'use client';

import { useState, useEffect } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { Badge } from '@/components/ui/badge';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  Bot,
  Settings,
  History,
  AlertTriangle,
  CheckCircle,
  Clock,
  XCircle,
  Play,
  Pause,
  RotateCcw,
  Shield,
  Ban,
  TrendingUp,
  TrendingDown,
  Info,
} from 'lucide-react';
import { api } from '@/lib/api';
import { useToastStore } from '@/stores/toast-store';
import type { RotationHistoryEntry, WalletBan } from '@/types/api';
import { formatDistanceToNow } from 'date-fns';

interface AutomationSettings {
  auto_select_enabled: boolean;
  auto_demote_enabled: boolean;
  probation_days: number;
  max_pinned_wallets: number;
  allocation_strategy: 'equal' | 'confidence_weighted' | 'performance';
}

interface AutomationPanelProps {
  workspaceId: string;
  onRefresh?: () => void;
}

export function AutomationPanel({ workspaceId, onRefresh }: AutomationPanelProps) {
  const [settings, setSettings] = useState<AutomationSettings>({
    auto_select_enabled: true,
    auto_demote_enabled: true,
    probation_days: 7,
    max_pinned_wallets: 3,
    allocation_strategy: 'confidence_weighted',
  });
  const [history, setHistory] = useState<RotationHistoryEntry[]>([]);
  const [bans, setBans] = useState<WalletBan[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const { addToast } = useToastStore();

  useEffect(() => {
    loadHistory();
    loadBans();
  }, [workspaceId]);

  const loadHistory = async () => {
    try {
      const data = await api.listRotationHistory({ limit: 20 });
      setHistory(data);
    } catch (error) {
      console.error('Failed to load rotation history:', error);
    }
  };

  const loadBans = async () => {
    try {
      const data = await api.listBans();
      setBans(data.bans);
    } catch (error) {
      console.error('Failed to load bans:', error);
    }
  };

  const handleToggleAutoSelect = async () => {
    try {
      setSettings((prev) => ({ ...prev, auto_select_enabled: !prev.auto_select_enabled }));
      addToast({
        type: 'success',
        title: `Auto-select ${!settings.auto_select_enabled ? 'enabled' : 'disabled'}`,
      });
    } catch (error) {
      addToast({ type: 'error', title: 'Failed to update setting' });
    }
  };

  const handleToggleAutoDemote = async () => {
    try {
      setSettings((prev) => ({ ...prev, auto_demote_enabled: !prev.auto_demote_enabled }));
      addToast({
        type: 'success',
        title: `Auto-demote ${!settings.auto_demote_enabled ? 'enabled' : 'disabled'}`,
      });
    } catch (error) {
      addToast({ type: 'error', title: 'Failed to update setting' });
    }
  };

  const handleTriggerOptimization = async () => {
    setIsLoading(true);
    try {
      await api.triggerOptimization();
      addToast({ type: 'success', title: 'Optimization triggered successfully' });
      loadHistory();
      onRefresh?.();
    } catch (error) {
      addToast({ type: 'error', title: 'Failed to trigger optimization' });
    } finally {
      setIsLoading(false);
    }
  };

  const handleUnban = async (address: string) => {
    try {
      await api.unbanWallet(address);
      addToast({ type: 'success', title: 'Wallet unbanned' });
      loadBans();
    } catch (error) {
      addToast({ type: 'error', title: 'Failed to unban wallet' });
    }
  };

  const handleAcknowledge = async (id: string) => {
    try {
      await api.acknowledgeRotation(id);
      loadHistory();
    } catch (error) {
      addToast({ type: 'error', title: 'Failed to acknowledge entry' });
    }
  };

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
        <div className="flex items-center justify-between">
          <CardTitle className="flex items-center gap-2 text-lg">
            <Bot className="h-5 w-5 text-primary" />
            Automation
          </CardTitle>
          <Button
            variant="outline"
            size="sm"
            onClick={handleTriggerOptimization}
            disabled={isLoading}
          >
            {isLoading ? (
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
          <TabsList className="grid w-full grid-cols-3">
            <TabsTrigger value="settings" className="text-xs">
              <Settings className="h-3 w-3 mr-1" />
              Settings
            </TabsTrigger>
            <TabsTrigger value="history" className="text-xs">
              <History className="h-3 w-3 mr-1" />
              History
            </TabsTrigger>
            <TabsTrigger value="bans" className="text-xs">
              <Ban className="h-3 w-3 mr-1" />
              Bans ({bans.length})
            </TabsTrigger>
          </TabsList>

          <TabsContent value="settings" className="mt-4 space-y-4">
            <div className="flex items-center justify-between py-2">
              <div className="space-y-0.5">
                <div className="text-sm font-medium">Auto-Select</div>
                <div className="text-xs text-muted-foreground">
                  Automatically fill empty slots with top performers
                </div>
              </div>
              <Switch
                checked={settings.auto_select_enabled}
                onCheckedChange={handleToggleAutoSelect}
              />
            </div>

            <div className="flex items-center justify-between py-2">
              <div className="space-y-0.5">
                <div className="text-sm font-medium">Auto-Demote</div>
                <div className="text-xs text-muted-foreground">
                  Automatically remove underperforming wallets
                </div>
              </div>
              <Switch
                checked={settings.auto_demote_enabled}
                onCheckedChange={handleToggleAutoDemote}
              />
            </div>

            <div className="pt-4 border-t border-border/50">
              <div className="text-xs text-muted-foreground mb-2">Thresholds</div>
              <div className="space-y-2 text-sm">
                <div className="flex justify-between">
                  <span className="text-muted-foreground">Probation Period</span>
                  <span>{settings.probation_days} days</span>
                </div>
                <div className="flex justify-between">
                  <span className="text-muted-foreground">Max Pins</span>
                  <span>{settings.max_pinned_wallets}</span>
                </div>
                <div className="flex justify-between items-center">
                  <span className="text-muted-foreground">Strategy</span>
                  <Badge variant="outline" className="text-xs capitalize">
                    {settings.allocation_strategy.replace(/_/g, ' ')}
                  </Badge>
                </div>
              </div>
            </div>
          </TabsContent>

          <TabsContent value="history" className="mt-4">
            <div className="space-y-2 max-h-64 overflow-y-auto">
              {history.length === 0 ? (
                <div className="text-center py-8 text-muted-foreground text-sm">
                  No automation history yet
                </div>
              ) : (
                history.map((entry) => (
                  <div
                    key={entry.id}
                    className={`flex items-start gap-3 p-3 rounded-lg bg-background/50 border ${
                      !entry.acknowledged ? 'border-primary/20' : 'border-border/50'
                    }`}
                  >
                    <div className="mt-0.5">{getActionIcon(entry.action)}</div>
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2 mb-1">
                        <Badge className={`text-xs ${getActionBadge(entry.action)}`}>
                          {entry.action.replace(/_/g, ' ')}
                        </Badge>
                        <span className="text-xs text-muted-foreground">
                          {formatDistanceToNow(new Date(entry.created_at), { addSuffix: true })}
                        </span>
                      </div>
                      <p className="text-sm truncate">{entry.reason}</p>
                      {(entry.wallet_in || entry.wallet_out) && (
                        <div className="text-xs text-muted-foreground mt-1 font-mono">
                          {entry.wallet_in && (
                            <span className="text-green-500">
                              +{entry.wallet_in.slice(0, 8)}...
                            </span>
                          )}
                          {entry.wallet_in && entry.wallet_out && ' / '}
                          {entry.wallet_out && (
                            <span className="text-red-500">
                              -{entry.wallet_out.slice(0, 8)}...
                            </span>
                          )}
                        </div>
                      )}
                    </div>
                    {!entry.acknowledged && (
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => handleAcknowledge(entry.id)}
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
            <div className="space-y-2 max-h-64 overflow-y-auto">
              {bans.length === 0 ? (
                <div className="text-center py-8 text-muted-foreground text-sm">
                  No banned wallets
                </div>
              ) : (
                bans.map((ban) => (
                  <div
                    key={ban.id}
                    className="flex items-center justify-between p-3 rounded-lg bg-background/50 border border-border/50"
                  >
                    <div className="flex items-center gap-3">
                      <Ban className="h-4 w-4 text-red-500" />
                      <div>
                        <p className="text-sm font-mono">
                          {ban.wallet_address.slice(0, 10)}...{ban.wallet_address.slice(-8)}
                        </p>
                        {ban.reason && (
                          <p className="text-xs text-muted-foreground">{ban.reason}</p>
                        )}
                        {ban.expires_at && (
                          <p className="text-xs text-muted-foreground">
                            Expires{' '}
                            {formatDistanceToNow(new Date(ban.expires_at), { addSuffix: true })}
                          </p>
                        )}
                      </div>
                    </div>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => handleUnban(ban.wallet_address)}
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
