'use client';

import { useState, useEffect } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { Input } from '@/components/ui/input';
import { useSettingsStore, Theme } from '@/stores/settings-store';
import { useToastStore } from '@/stores/toast-store';
import {
  RefreshCw,
  Wallet,
  Shield,
  Palette,
  Save,
  Check,
  AlertTriangle,
  Users,
  ChevronRight,
  UserPlus,
  Link as LinkIcon,
  ExternalLink,
  Star,
  Trash2,
  Settings2,
  ArrowUpRight,
} from 'lucide-react';
import { useAuthStore } from '@/stores/auth-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { InviteMemberDialog } from '@/components/workspace/InviteMemberDialog';
import { MemberList } from '@/components/workspace/MemberList';
import { ConnectWalletModal } from '@/components/wallet/ConnectWalletModal';
import { InfoTooltip } from '@/components/shared/InfoTooltip';
import { PageIntro } from '@/components/shared/PageIntro';
import { useWalletStore } from '@/stores/wallet-store';
import {
  useCreateWalletWithdrawalMutation,
  useWalletBalanceQuery,
  useWalletWithdrawalsQuery,
} from '@/hooks/queries/useWalletsQuery';
import api from '@/lib/api';
import { queryKeys } from '@/lib/queryClient';
import Link from 'next/link';
import { formatCurrency, formatTimeAgo } from '@/lib/utils';

export default function SettingsPage() {
  const toast = useToastStore();
  const queryClient = useQueryClient();
  const { user } = useAuthStore();
  const { currentWorkspace, setCurrentWorkspace } = useWorkspaceStore();
  const { appearance, updateAppearance } = useSettingsStore();

  const [connectWalletOpen, setConnectWalletOpen] = useState(false);
  const [inviteDialogOpen, setInviteDialogOpen] = useState(false);
  const [walletConnectProjectId, setWalletConnectProjectId] = useState('');
  const [isSavingWalletConnect, setIsSavingWalletConnect] = useState(false);
  const [isSavingTradingConfig, setIsSavingTradingConfig] = useState(false);
  const [polygonRpcUrl, setPolygonRpcUrl] = useState('');
  const [alchemyApiKey, setAlchemyApiKey] = useState('');
  const [arbAutoExecute, setArbAutoExecute] = useState(false);
  const [liveTradingEnabled, setLiveTradingEnabled] = useState(false);
  const [exitHandlerEnabled, setExitHandlerEnabled] = useState(false);
  const [arbAutoExecuteDirty, setArbAutoExecuteDirty] = useState(false);
  const [liveTradingEnabledDirty, setLiveTradingEnabledDirty] = useState(false);
  const [exitHandlerEnabledDirty, setExitHandlerEnabledDirty] = useState(false);

  // Risk management form state (backed by API, not localStorage)
  const [isSavingRisk, setIsSavingRisk] = useState(false);
  const [riskMaxDailyLoss, setRiskMaxDailyLoss] = useState<number | ''>('');
  const [riskMaxDrawdownPct, setRiskMaxDrawdownPct] = useState<number | ''>('');
  const [riskMaxConsecutiveLosses, setRiskMaxConsecutiveLosses] = useState<number | ''>('');
  const [riskCooldownMinutes, setRiskCooldownMinutes] = useState<number | ''>('');
  const [riskEnabled, setRiskEnabled] = useState(true);
  const [isTrippingCb, setIsTrippingCb] = useState(false);
  const [isResettingCb, setIsResettingCb] = useState(false);
  const [withdrawDestination, setWithdrawDestination] = useState('');
  const [withdrawAmount, setWithdrawAmount] = useState('');

  const {
    connectedWallets,
    primaryWallet,
    isLoading: walletLoading,
    fetchWallets,
    setPrimary,
    disconnectWallet,
  } = useWalletStore();

  // Get current user's role in workspace
  const currentUserRole = currentWorkspace?.my_role;
  const canInvite = currentUserRole === 'owner' || currentUserRole === 'admin';
  const canManageRisk = currentUserRole === 'owner' || currentUserRole === 'admin';
  const canConfigureWalletConnect = currentUserRole === 'owner' || currentUserRole === 'admin';
  const canManageWithdrawals = currentUserRole === 'owner' || currentUserRole === 'admin';

  // Fetch workspace members
  const { data: members = [], isLoading: membersLoading } = useQuery({
    queryKey: ['workspace', currentWorkspace?.id, 'members'],
    queryFn: () => api.listWorkspaceMembers(currentWorkspace!.id),
    enabled: !!currentWorkspace?.id,
  });

  // Fetch pending invites
  const { data: invites = [] } = useQuery({
    queryKey: ['workspace', currentWorkspace?.id, 'invites'],
    queryFn: () => api.listWorkspaceInvites(currentWorkspace!.id),
    enabled: !!currentWorkspace?.id && canInvite,
  });

  // Fetch service status
  const { data: serviceStatus } = useQuery({
    queryKey: queryKeys.runtime.serviceStatus(),
    queryFn: () => api.getServiceStatus(currentWorkspace!.id),
    enabled: !!currentWorkspace?.id && canConfigureWalletConnect,
    refetchInterval: 30000,
  });

  // Fetch risk status (real CB config from backend)
  const { data: riskStatus } = useQuery({
    queryKey: queryKeys.risk.status(),
    queryFn: () => api.getRiskStatus(currentWorkspace!.id),
    enabled: !!currentWorkspace?.id,
    refetchInterval: 30000,
  });
  const primaryConnectedWallet =
    connectedWallets.find((wallet) => wallet.address === primaryWallet) ?? null;
  const {
    data: primaryWalletBalance,
    isPending: isPrimaryWalletBalancePending,
  } = useWalletBalanceQuery(primaryWallet ?? null);
  const { data: recentWithdrawals = [], isLoading: withdrawalsLoading } =
    useWalletWithdrawalsQuery(8);
  const createWithdrawalMutation = useCreateWalletWithdrawalMutation();

  // Initialize risk form from API data
  useEffect(() => {
    if (riskStatus?.circuit_breaker?.config) {
      const cfg = riskStatus.circuit_breaker.config;
      setRiskMaxDailyLoss(cfg.max_daily_loss);
      setRiskMaxDrawdownPct(Math.round(cfg.max_drawdown_pct * 10000) / 100); // ratio -> percent display
      setRiskMaxConsecutiveLosses(cfg.max_consecutive_losses);
      setRiskCooldownMinutes(cfg.cooldown_minutes);
      setRiskEnabled(cfg.enabled);
    }
  }, [riskStatus]);

  // Initialize walletConnectProjectId from workspace
  useEffect(() => {
    if (currentWorkspace?.walletconnect_project_id) {
      setWalletConnectProjectId(currentWorkspace.walletconnect_project_id);
    }
  }, [currentWorkspace?.walletconnect_project_id]);

  // Initialize trading config from workspace
  useEffect(() => {
    if (currentWorkspace) {
      setPolygonRpcUrl(currentWorkspace.polygon_rpc_url || '');
      setAlchemyApiKey(''); // Never pre-fill masked key
      setArbAutoExecute(currentWorkspace.arb_auto_execute ?? false);
      setLiveTradingEnabled(currentWorkspace.live_trading_enabled ?? false);
      setExitHandlerEnabled(currentWorkspace.exit_handler_enabled ?? false);
      setArbAutoExecuteDirty(false);
      setLiveTradingEnabledDirty(false);
      setExitHandlerEnabledDirty(false);
    }
  }, [currentWorkspace]);

  useEffect(() => {
    fetchWallets().catch(() => {
      // Wallet card will show empty/fallback state if loading fails.
    });
  }, [fetchWallets]);

  // Revoke invite mutation
  const handleRevokeInvite = async (inviteId: string) => {
    try {
      await api.revokeInvite(currentWorkspace!.id, inviteId);
      queryClient.invalidateQueries({ queryKey: ['workspace', currentWorkspace?.id, 'invites'] });
      toast.success('Invite revoked', 'The invitation has been cancelled');
    } catch (err) {
      toast.error('Failed to revoke invite', err instanceof Error ? err.message : 'Unknown error');
    }
  };

  // Save WalletConnect project ID
  const handleSaveWalletConnect = async () => {
    if (!currentWorkspace) return;
    setIsSavingWalletConnect(true);
    try {
      await api.updateWorkspace(currentWorkspace.id, {
        walletconnect_project_id: walletConnectProjectId || undefined,
      });
      toast.success('WalletConnect settings saved', 'Your wallet connection is now configured');
      queryClient.invalidateQueries({ queryKey: ['workspace'] });
    } catch (err) {
      toast.error('Failed to save', err instanceof Error ? err.message : 'Unknown error');
    } finally {
      setIsSavingWalletConnect(false);
    }
  };

  // Save trading configuration
  const handleSaveTradingConfig = async () => {
    if (!currentWorkspace) return;
    setIsSavingTradingConfig(true);
    try {
      const updates: Record<string, unknown> = {};
      if (arbAutoExecuteDirty) updates.arb_auto_execute = arbAutoExecute;
      if (liveTradingEnabledDirty) updates.live_trading_enabled = liveTradingEnabled;
      if (exitHandlerEnabledDirty) updates.exit_handler_enabled = exitHandlerEnabled;
      if (polygonRpcUrl) updates.polygon_rpc_url = polygonRpcUrl;
      if (alchemyApiKey) updates.alchemy_api_key = alchemyApiKey;
      if (Object.keys(updates).length === 0) {
        toast.warning('No changes to save', 'Trading settings already match the current workspace');
        return;
      }
      const updatedWorkspace = await api.updateWorkspace(currentWorkspace.id, updates);
      setCurrentWorkspace(updatedWorkspace);
      toast.success('Trading config saved', 'Your trading configuration has been updated');
      queryClient.invalidateQueries({ queryKey: ['workspace'] });
      queryClient.invalidateQueries({ queryKey: queryKeys.runtime.serviceStatus() });
      setAlchemyApiKey(''); // Clear after save
    } catch (err) {
      toast.error('Failed to save', err instanceof Error ? err.message : 'Unknown error');
    } finally {
      setIsSavingTradingConfig(false);
    }
  };

  // Save risk management config (real API call)
  const handleSaveRisk = async () => {
    if (!currentWorkspace) return;
    setIsSavingRisk(true);
    try {
      await api.updateCircuitBreakerConfig(currentWorkspace.id, {
        max_daily_loss: riskMaxDailyLoss === '' ? undefined : Number(riskMaxDailyLoss),
        max_drawdown_pct: riskMaxDrawdownPct === '' ? undefined : Number(riskMaxDrawdownPct) / 100, // percent -> ratio
        max_consecutive_losses: riskMaxConsecutiveLosses === '' ? undefined : Number(riskMaxConsecutiveLosses),
        cooldown_minutes: riskCooldownMinutes === '' ? undefined : Number(riskCooldownMinutes),
        enabled: riskEnabled,
      });
      toast.success('Risk settings saved', 'Circuit breaker configuration has been updated');
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.status() });
    } catch (err) {
      toast.error('Failed to save risk settings', err instanceof Error ? err.message : 'Unknown error');
    } finally {
      setIsSavingRisk(false);
    }
  };

  const handleTripCircuitBreaker = async () => {
    if (!currentWorkspace) return;
    setIsTrippingCb(true);
    try {
      await api.manualTripCircuitBreaker(currentWorkspace.id);
      toast.success('Circuit breaker tripped', 'All automated trading has been paused');
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.status() });
    } catch (err) {
      toast.error('Failed to trip circuit breaker', err instanceof Error ? err.message : 'Unknown error');
    } finally {
      setIsTrippingCb(false);
    }
  };

  const handleResetCircuitBreaker = async () => {
    if (!currentWorkspace) return;
    setIsResettingCb(true);
    try {
      await api.resetCircuitBreaker(currentWorkspace.id);
      toast.success('Circuit breaker reset', 'Automated trading has been resumed');
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.status() });
    } catch (err) {
      toast.error('Failed to reset circuit breaker', err instanceof Error ? err.message : 'Unknown error');
    } finally {
      setIsResettingCb(false);
    }
  };

  // Check if risk form has changed from API baseline
  const riskDirty = riskStatus?.circuit_breaker?.config
    ? (riskMaxDailyLoss !== riskStatus.circuit_breaker.config.max_daily_loss ||
       riskMaxDrawdownPct !== Math.round(riskStatus.circuit_breaker.config.max_drawdown_pct * 10000) / 100 ||
       riskMaxConsecutiveLosses !== riskStatus.circuit_breaker.config.max_consecutive_losses ||
       riskCooldownMinutes !== riskStatus.circuit_breaker.config.cooldown_minutes ||
       riskEnabled !== riskStatus.circuit_breaker.config.enabled)
    : false;

  const themeButtons: { value: Theme; label: string }[] = [
    { value: 'light', label: 'Light' },
    { value: 'dark', label: 'Dark' },
    { value: 'system', label: 'System' },
  ];

  const handleMakePrimary = async (address: string) => {
    try {
      await setPrimary(address);
      toast.success('Primary wallet updated', 'This wallet is now active for live trading');
    } catch (err) {
      toast.error(
        'Failed to set primary wallet',
        err instanceof Error ? err.message : 'Unknown error'
      );
    }
  };

  const handleDisconnectWallet = async (address: string) => {
    try {
      await disconnectWallet(address);
      toast.success('Wallet disconnected', 'Wallet removed from vault');
    } catch (err) {
      toast.error(
        'Failed to disconnect wallet',
        err instanceof Error ? err.message : 'Unknown error'
      );
    }
  };

  const handleWithdraw = async () => {
    if (!primaryWallet) {
      toast.error('No active wallet', 'Connect a wallet and mark it active before withdrawing.');
      return;
    }

    const amount = Number(withdrawAmount);
    if (!Number.isFinite(amount) || amount <= 0) {
      toast.error('Invalid amount', 'Enter a positive USDC amount to withdraw.');
      return;
    }

    if (!/^0x[a-fA-F0-9]{40}$/.test(withdrawDestination.trim())) {
      toast.error('Invalid destination', 'Enter a valid Polygon wallet address.');
      return;
    }

    if (withdrawDestination.trim().toLowerCase() === primaryWallet.toLowerCase()) {
      toast.error(
        'Same wallet selected',
        'The destination matches your active trading wallet. If this is already your MetaMask address, no transfer is needed.',
      );
      return;
    }

    try {
      await createWithdrawalMutation.mutateAsync({
        source_address: primaryWallet,
        destination_address: withdrawDestination.trim(),
        amount,
      });
      setWithdrawDestination('');
      setWithdrawAmount('');
    } catch (err) {
      toast.error(
        'Withdrawal failed',
        err instanceof Error ? err.message : 'Unable to submit withdrawal',
      );
    }
  };

  const shortAddress = (address: string) => `${address.slice(0, 6)}...${address.slice(-4)}`;

  return (
    <div className="mx-auto max-w-3xl space-y-5 sm:space-y-6">
      {/* Page Header */}
      <div>
        <h1 className="text-2xl font-bold tracking-tight sm:text-3xl">Settings</h1>
        <p className="text-muted-foreground">
          Manage your team, wallet setup, automation, and safety controls
        </p>
      </div>

      <PageIntro
        title="What you can change here"
        description="This page controls who can access the workspace, which wallet the bot can trade with, whether automated trading is allowed, and what safety limits stop the system."
        bullets={[
          "Account and Wallet sections control where live trades are sent.",
          "Trading Configuration controls whether the system is allowed to place real trades automatically.",
          "Risk Management defines the limits that pause trading when losses or instability appear."
        ]}
      />

      {/* Account */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Wallet className="h-5 w-5" />
            Account
          </CardTitle>
          <CardDescription>
            Choose which wallet the bot is allowed to use for live trading and withdraw settled USDC when needed
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="rounded-lg border p-4">
            <div className="mb-4 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="flex items-center gap-2 font-medium">
                  <span>Connected Wallets</span>
                  <InfoTooltip content="These are the wallets connected under your account. The wallet marked Active is the one the bot will use for live trading." />
                </p>
                <p className="text-sm text-muted-foreground">
                  The wallet marked Active is used automatically for live trading
                </p>
              </div>
              <Button onClick={() => setConnectWalletOpen(true)} className="w-full sm:w-auto">Add Wallet</Button>
            </div>

            {walletLoading && connectedWallets.length === 0 ? (
              <p className="text-sm text-muted-foreground">Loading wallets...</p>
            ) : connectedWallets.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No wallets connected yet. Add one before turning on live trading.
              </p>
            ) : (
              <div className="space-y-2">
                {connectedWallets.map((wallet) => {
                  const isPrimary = wallet.address === primaryWallet;
                  return (
                    <div
                      key={wallet.id}
                      className="flex flex-col gap-3 rounded-md border p-3 sm:flex-row sm:items-center sm:justify-between"
                    >
                      <div className="min-w-0">
                        <p className="font-medium truncate">
                          {wallet.label || shortAddress(wallet.address)}
                        </p>
                        <p className="text-xs text-muted-foreground font-mono truncate">
                          {wallet.address}
                        </p>
                      </div>
                      <div className="flex items-center gap-2 self-start sm:self-auto">
                        {isPrimary ? (
                          <span className="inline-flex items-center gap-1 rounded-full border border-green-500/30 bg-green-500/10 px-2 py-1 text-xs text-green-600">
                            <Star className="h-3 w-3" />
                            Active
                          </span>
                        ) : (
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={() => handleMakePrimary(wallet.address)}
                            disabled={walletLoading}
                          >
                            Use for Trading
                          </Button>
                        )}
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => handleDisconnectWallet(wallet.address)}
                          disabled={walletLoading}
                          aria-label={`Disconnect wallet ${wallet.label || shortAddress(wallet.address)}`}
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          <div className="rounded-lg border p-4">
            <div className="mb-4">
              <p className="flex items-center gap-2 font-medium">
                <span>Withdraw USDC</span>
                <InfoTooltip content="This sends a real on-chain Polygon USDC.e transfer from the active trading wallet. It only moves free USDC, not open position inventory." />
              </p>
              <p className="text-sm text-muted-foreground">
                Send settled USDC from the active trading wallet to another Polygon address, such as a different MetaMask wallet.
              </p>
            </div>

            {!canManageWithdrawals ? (
              <p className="text-sm text-muted-foreground">
                Only workspace owners and admins can initiate withdrawals.
              </p>
            ) : !primaryWallet ? (
              <p className="text-sm text-muted-foreground">
                Connect and activate a wallet before withdrawing funds.
              </p>
            ) : (
              <div className="space-y-4">
                <div className="rounded-md border bg-muted/30 p-3 text-sm">
                  <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                    <div>
                      <p className="font-medium">
                        Source wallet: {primaryConnectedWallet?.label || shortAddress(primaryWallet)}
                      </p>
                      <p className="font-mono text-xs text-muted-foreground">
                        {primaryWallet}
                      </p>
                    </div>
                    <div className="text-sm font-medium">
                      {isPrimaryWalletBalancePending
                        ? 'Loading balance...'
                        : primaryWalletBalance
                          ? `${formatCurrency(primaryWalletBalance.usdc_balance)} available`
                          : 'Balance unavailable'}
                    </div>
                  </div>
                </div>

                <div className="grid gap-3 md:grid-cols-2">
                  <label className="space-y-2 text-sm">
                    <span className="font-medium">Destination Address</span>
                    <Input
                      value={withdrawDestination}
                      onChange={(e) => setWithdrawDestination(e.target.value)}
                      placeholder="0x..."
                      className="font-mono"
                    />
                  </label>
                  <label className="space-y-2 text-sm">
                    <span className="font-medium">Amount (USDC)</span>
                    <Input
                      type="number"
                      min="0"
                      step="0.000001"
                      value={withdrawAmount}
                      onChange={(e) => setWithdrawAmount(e.target.value)}
                      placeholder="25.00"
                    />
                  </label>
                </div>

                <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                  <p className="text-xs text-muted-foreground">
                    If your MetaMask wallet already uses this same active address, the funds are already there and no withdrawal transfer is needed.
                  </p>
                  <Button
                    onClick={handleWithdraw}
                    disabled={createWithdrawalMutation.isPending || !withdrawDestination || !withdrawAmount}
                    className="w-full sm:w-auto"
                  >
                    {createWithdrawalMutation.isPending ? (
                      <RefreshCw className="h-4 w-4 animate-spin" />
                    ) : (
                      <ArrowUpRight className="h-4 w-4" />
                    )}
                    <span className="ml-2">Withdraw</span>
                  </Button>
                </div>

                <div className="space-y-2">
                  <div className="flex items-center justify-between">
                    <p className="text-sm font-medium">Recent Withdrawals</p>
                    {withdrawalsLoading && (
                      <span className="text-xs text-muted-foreground">Refreshing...</span>
                    )}
                  </div>
                  {recentWithdrawals.length === 0 ? (
                    <p className="text-sm text-muted-foreground">
                      No withdrawals recorded yet.
                    </p>
                  ) : (
                    <div className="space-y-2">
                      {recentWithdrawals.map((withdrawal) => (
                        <div
                          key={withdrawal.id}
                          className="flex flex-col gap-2 rounded-md border p-3 sm:flex-row sm:items-center sm:justify-between"
                        >
                          <div className="min-w-0">
                            <p className="font-medium">
                              {formatCurrency(withdrawal.amount)} to {shortAddress(withdrawal.destination_address)}
                            </p>
                            <p className="text-xs text-muted-foreground">
                              {formatTimeAgo(withdrawal.requested_at)} · {withdrawal.status}
                            </p>
                            {withdrawal.error && (
                              <p className="mt-1 text-xs text-destructive">
                                {withdrawal.error}
                              </p>
                            )}
                          </div>
                          <div className="flex items-center gap-2">
                            {withdrawal.tx_hash && (
                              <a
                                href={withdrawal.explorer_url}
                                target="_blank"
                                rel="noopener noreferrer"
                                className="inline-flex items-center gap-1 text-xs text-primary hover:underline"
                              >
                                View tx
                                <ExternalLink className="h-3 w-3" />
                              </a>
                            )}
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              </div>
            )}
          </div>
        </CardContent>
      </Card>

      {/* WalletConnect Settings (Owner/Admin only) */}
      {currentWorkspace && canConfigureWalletConnect && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <LinkIcon className="h-5 w-5" />
              Wallet Connection
            </CardTitle>
            <CardDescription>
              Set up browser wallet sign-in for people who use MetaMask or other WalletConnect-compatible wallets
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2">
              <label className="inline-flex items-center gap-1 text-sm font-medium" htmlFor="walletconnect-project-id">
                WalletConnect Project ID
                <InfoTooltip content="This is the app identifier from WalletConnect Cloud. Without it, browser wallets cannot connect to this dashboard." />
              </label>
              <div className="flex flex-col gap-2 sm:flex-row">
                <input
                  id="walletconnect-project-id"
                  type="text"
                  value={walletConnectProjectId}
                  onChange={(e) => setWalletConnectProjectId(e.target.value)}
                  placeholder="Enter your WalletConnect project ID"
                  className="flex-1 rounded border bg-background px-3 py-2 text-sm"
                />
                <Button
                  onClick={handleSaveWalletConnect}
                  disabled={isSavingWalletConnect || walletConnectProjectId === (currentWorkspace.walletconnect_project_id || '')}
                  className="w-full sm:w-auto"
                >
                  {isSavingWalletConnect ? (
                    <RefreshCw className="h-4 w-4 animate-spin" />
                  ) : (
                    <Save className="h-4 w-4" />
                  )}
                </Button>
              </div>
              <p className="text-xs text-muted-foreground">
                Get your project ID from{' '}
                <a
                  href="https://cloud.walletconnect.com"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-primary hover:underline inline-flex items-center gap-1"
                >
                  WalletConnect Cloud
                  <ExternalLink className="h-3 w-3" />
                </a>
              </p>
            </div>

            {!walletConnectProjectId && !currentWorkspace.walletconnect_project_id && (
              <div className="rounded-lg border border-yellow-500/20 bg-yellow-500/10 p-3">
                <p className="text-sm text-yellow-500">
                  Browser wallet sign-in is not ready yet. Add a WalletConnect project ID to enable MetaMask and similar wallets.
                </p>
              </div>
            )}

            {(walletConnectProjectId || currentWorkspace.walletconnect_project_id) && (
              <div className="rounded-lg border border-green-500/20 bg-green-500/10 p-3">
                <p className="text-sm text-green-500 flex items-center gap-2">
                  <Check className="h-4 w-4" />
                  Browser wallet connection is configured.
                </p>
              </div>
            )}
          </CardContent>
        </Card>
      )}

      {/* Trading Configuration (Owner/Admin only) */}
      {currentWorkspace && canConfigureWalletConnect && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Settings2 className="h-5 w-5" />
              Trading Configuration
            </CardTitle>
            <CardDescription>
              Control the services and permissions the bot needs before it can trade automatically
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            {/* Service Status Indicators */}
            {serviceStatus && (
              <div className="rounded-lg border p-4 space-y-2">
                <p className="flex items-center gap-2 text-sm font-medium mb-3">
                  <span>Service Status</span>
                  <InfoTooltip content="These services must be healthy for automated trading to work correctly. A stopped service usually means the bot cannot monitor or execute trades reliably." />
                </p>
                {Object.entries(serviceStatus).map(([key, status]) => (
                  <div key={key} className="flex flex-col gap-1 text-sm sm:flex-row sm:items-center sm:justify-between">
                <span className="capitalize">{key.replace(/_/g, ' ')}</span>
                    <div className="flex items-center gap-2">
                      <span
                        className={`inline-block h-2 w-2 rounded-full ${
                          status.running ? 'bg-green-500' : 'bg-red-400'
                        }`}
                      />
                      <span className={status.running ? 'text-green-600' : 'text-muted-foreground'}>
                        {status.running ? 'Running' : status.reason || 'Stopped'}
                      </span>
                    </div>
                  </div>
                ))}
              </div>
            )}

            {/* Polygon RPC */}
            <div className="space-y-2">
              <label className="inline-flex items-center gap-1 text-sm font-medium" htmlFor="polygon-rpc-url">
                Polygon RPC URL
                <InfoTooltip content="This is the blockchain connection used to read wallet balances and send on-chain transactions. If it is missing or slow, live trading can fail." />
              </label>
              <input
                id="polygon-rpc-url"
                type="text"
                value={polygonRpcUrl}
                onChange={(e) => setPolygonRpcUrl(e.target.value)}
                placeholder="https://polygon-mainnet.g.alchemy.com/v2/..."
                className="w-full rounded border bg-background px-3 py-2 text-sm"
              />
              <p className="text-xs text-muted-foreground">
                Needed for wallet monitoring and blockchain operations. Accepted providers:
                Alchemy, Infura, Ankr, Polygon, LlamaRPC, DRPC, PublicNode, 1RPC,
                Tenderly, Particle Network.
              </p>
            </div>

            {/* Alchemy API Key */}
            <div className="space-y-2">
              <label className="inline-flex items-center gap-1 text-sm font-medium" htmlFor="alchemy-api-key">
                Alchemy API Key
                <InfoTooltip content="If you use Alchemy, you can supply the API key directly here instead of entering the full RPC URL." />
              </label>
              <input
                id="alchemy-api-key"
                type="password"
                value={alchemyApiKey}
                onChange={(e) => setAlchemyApiKey(e.target.value)}
                placeholder={currentWorkspace.alchemy_api_key || 'Enter Alchemy API key'}
                className="w-full rounded border bg-background px-3 py-2 text-sm"
              />
              <p className="text-xs text-muted-foreground">
                Optional shortcut for Alchemy users. Leave empty to keep the current key.
              </p>
            </div>

            {/* Arb Auto-Execute Toggle */}
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="font-medium">Arbitrage Auto-Execute</p>
                <p className="text-sm text-muted-foreground">
                  Let the bot place arbitrage trades automatically when it finds a qualified opportunity
                </p>
              </div>
              <Switch
                checked={arbAutoExecute}
                onCheckedChange={(checked) => {
                  setArbAutoExecute(checked);
                  setArbAutoExecuteDirty(true);
                }}
              />
            </div>

            {/* Exit Handler Toggle */}
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="font-medium">Exit Handler</p>
                <p className="text-sm text-muted-foreground">
                  Let the bot close positions automatically when an exit condition or market resolution is reached
                </p>
              </div>
              <Switch
                checked={exitHandlerEnabled}
                onCheckedChange={(checked) => {
                  setExitHandlerEnabled(checked);
                  setExitHandlerEnabledDirty(true);
                }}
              />
            </div>

            {/* Live Trading Toggle */}
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="font-medium">Live Trading</p>
                <p className="text-sm text-muted-foreground">
                  Allow the system to place real trades with the active wallet
                </p>
              </div>
              <Switch
                checked={liveTradingEnabled}
                onCheckedChange={(checked) => {
                  setLiveTradingEnabled(checked);
                  setLiveTradingEnabledDirty(true);
                }}
              />
            </div>
            {liveTradingEnabled && (
              <div className="rounded-lg border border-yellow-500/20 bg-yellow-500/10 p-3">
                <p className="text-sm text-yellow-500 flex items-center gap-2">
                  <AlertTriangle className="h-4 w-4" />
                  Live trading uses real funds. Only turn this on when the active wallet is correct and funded.
                </p>
              </div>
            )}

            <Button
              onClick={handleSaveTradingConfig}
              disabled={isSavingTradingConfig}
              className="w-full"
            >
              {isSavingTradingConfig ? (
                <>
                  <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
                  Saving...
                </>
              ) : (
                <>
                  <Save className="mr-2 h-4 w-4" />
                  Save Trading Settings
                </>
              )}
            </Button>
          </CardContent>
        </Card>
      )}

      {/* Team Management */}
      {currentWorkspace && (
        <Card>
          <CardHeader>
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <CardTitle className="flex items-center gap-2">
                  <Users className="h-5 w-5" />
                  Team
                </CardTitle>
                <CardDescription>
                  Control who can access this workspace and what they are allowed to do
                </CardDescription>
              </div>
              {canInvite && (
                <Button onClick={() => setInviteDialogOpen(true)}>
                  <UserPlus className="mr-2 h-4 w-4" />
                  Invite Teammate
                </Button>
              )}
            </div>
          </CardHeader>
          <CardContent className="space-y-6">
            {/* Members List */}
            <div>
              <h3 className="text-sm font-medium mb-3">Workspace Members ({members.length})</h3>
              {membersLoading ? (
                <div className="text-center py-4 text-muted-foreground">Loading...</div>
              ) : (
                <MemberList
                  workspaceId={currentWorkspace.id}
                  members={members}
                  currentUserRole={currentUserRole}
                />
              )}
            </div>

            {/* Pending Invites (only show if canInvite and has pending) */}
            {canInvite && invites.filter(i => !i.accepted_at).length > 0 && (
              <div>
                <h3 className="text-sm font-medium mb-3">Pending Invitations</h3>
                <div className="space-y-2">
                  {invites.filter(i => !i.accepted_at).map(invite => (
                    <div key={invite.id} className="flex flex-col gap-2 rounded-lg border p-3 sm:flex-row sm:items-center sm:justify-between">
                      <div>
                        <p className="font-medium">{invite.email}</p>
                        <p className="text-xs text-muted-foreground">
                          {invite.role} · Expires {new Date(invite.expires_at).toLocaleDateString()}
                        </p>
                      </div>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => handleRevokeInvite(invite.id)}
                      >
                        Revoke
                      </Button>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </CardContent>
        </Card>
      )}

      {/* User Management (Platform Admin Only) */}
      {user?.role === 'PlatformAdmin' && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Users className="h-5 w-5" />
              User Management
            </CardTitle>
            <CardDescription>
              Platform-wide account management for administrators
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Link href="/settings/users">
              <div className="flex items-center justify-between rounded-lg border p-4 transition-colors hover:bg-muted/50 cursor-pointer">
                <div>
                  <p className="font-medium">Manage Users</p>
                  <p className="text-sm text-muted-foreground">
                    Create, edit, and remove platform user accounts
                  </p>
                </div>
                <ChevronRight className="h-5 w-5 text-muted-foreground" />
              </div>
            </Link>
          </CardContent>
        </Card>
      )}

      {/* Risk Management — backed by real API */}
      {currentWorkspace && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Shield className="h-5 w-5" />
              Risk Management
            </CardTitle>
            <CardDescription>
              Safety limits that pause automated trading when losses or instability exceed your limits
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            {!riskStatus ? (
              <div className="text-center py-4 text-muted-foreground">Loading risk configuration...</div>
            ) : (
              <>
                {/* Circuit Breaker Status Banner */}
                {riskStatus.circuit_breaker.tripped && (
                  <div className="rounded-lg border border-red-500/20 bg-red-500/10 p-3">
                    <p className="text-sm text-red-500 flex items-center gap-2">
                      <AlertTriangle className="h-4 w-4" />
                      Safety stop is active and automated trading is paused
                      {riskStatus.circuit_breaker.trip_reason && (
                        <> (reason: {riskStatus.circuit_breaker.trip_reason})</>
                      )}
                    </p>
                  </div>
                )}

                {/* Max Daily Loss */}
                <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                  <div>
                    <p className="flex items-center gap-2 font-medium">
                      <span>Max Daily Loss</span>
                      <InfoTooltip content="If losses for the day pass this amount, the system pauses automated trading." />
                    </p>
                    <p className="text-sm text-muted-foreground">
                      Pause automated trading when daily losses pass this amount
                    </p>
                  </div>
                  <div className="flex items-center gap-2">
                    <span className="text-muted-foreground">$</span>
                    <input
                      type="number"
                      value={riskMaxDailyLoss}
                      onChange={(e) => setRiskMaxDailyLoss(e.target.value === '' ? '' : Number(e.target.value))}
                      className="w-28 rounded border bg-background px-3 py-1 text-right"
                      min={0}
                      step={100}
                      disabled={!canManageRisk}
                    />
                  </div>
                </div>

                {/* Max Drawdown */}
                <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                  <div>
                    <p className="flex items-center gap-2 font-medium">
                      <span>Max Drawdown</span>
                      <InfoTooltip content="Drawdown measures how far the portfolio has fallen from its recent peak. If the drop gets too large, the system pauses trading." />
                    </p>
                    <p className="text-sm text-muted-foreground">
                      Pause trading when portfolio value falls this far from its recent peak
                    </p>
                  </div>
                  <div className="flex items-center gap-2">
                    <input
                      type="number"
                      value={riskMaxDrawdownPct}
                      onChange={(e) => setRiskMaxDrawdownPct(e.target.value === '' ? '' : Number(e.target.value))}
                      className="w-20 rounded border bg-background px-3 py-1 text-right"
                      min={1}
                      max={100}
                      step={1}
                      disabled={!canManageRisk}
                    />
                    <span className="text-muted-foreground">%</span>
                  </div>
                </div>

                {/* Max Consecutive Losses */}
                <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                  <div>
                    <p className="font-medium">Max Consecutive Losses</p>
                    <p className="text-sm text-muted-foreground">
                      Trips after this many consecutive losing trades
                    </p>
                  </div>
                  <input
                    type="number"
                    value={riskMaxConsecutiveLosses}
                    onChange={(e) => setRiskMaxConsecutiveLosses(e.target.value === '' ? '' : Number(e.target.value))}
                    className="w-20 rounded border bg-background px-3 py-1 text-right"
                    min={1}
                    max={100}
                    disabled={!canManageRisk}
                  />
                </div>

                {/* Cooldown Minutes */}
                <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                  <div>
                    <p className="font-medium">Cooldown Period</p>
                    <p className="text-sm text-muted-foreground">
                      Minutes to wait before auto-resuming after a trip
                    </p>
                  </div>
                  <div className="flex items-center gap-2">
                    <input
                      type="number"
                      value={riskCooldownMinutes}
                      onChange={(e) => setRiskCooldownMinutes(e.target.value === '' ? '' : Number(e.target.value))}
                      className="w-20 rounded border bg-background px-3 py-1 text-right"
                      min={0}
                      max={1440}
                      disabled={!canManageRisk}
                    />
                    <span className="text-muted-foreground">min</span>
                  </div>
                </div>

                {/* Circuit Breaker Enabled */}
                <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                  <div>
                    <p className="font-medium">Circuit Breaker</p>
                    <p className="text-sm text-muted-foreground">
                      Pause trading when loss thresholds are exceeded
                    </p>
                  </div>
                  <Switch
                    checked={riskEnabled}
                    onCheckedChange={setRiskEnabled}
                    disabled={!canManageRisk}
                  />
                </div>

                {/* Manual Trip / Reset Buttons */}
                {canManageRisk && (
                  <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                    <div>
                      <p className="font-medium">Manual Override</p>
                      <p className="text-sm text-muted-foreground">
                        Immediately trip or reset the circuit breaker
                      </p>
                    </div>
                    <div className="flex gap-2">
                      <Button
                        variant="destructive"
                        size="sm"
                        onClick={handleTripCircuitBreaker}
                        disabled={isTrippingCb || riskStatus?.circuit_breaker?.tripped}
                      >
                        {isTrippingCb ? (
                          <><RefreshCw className="mr-1 h-3 w-3 animate-spin" />Tripping...</>
                        ) : (
                          <><AlertTriangle className="mr-1 h-3 w-3" />Trip</>
                        )}
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={handleResetCircuitBreaker}
                        disabled={isResettingCb || !riskStatus?.circuit_breaker?.tripped}
                      >
                        {isResettingCb ? (
                          <><RefreshCw className="mr-1 h-3 w-3 animate-spin" />Resetting...</>
                        ) : (
                          <><Shield className="mr-1 h-3 w-3" />Reset</>
                        )}
                      </Button>
                    </div>
                  </div>
                )}

                {/* Current Runtime Stats */}
                <div className="rounded-lg border p-4 space-y-1">
                  <p className="text-sm font-medium mb-2">Current Status</p>
                  <div className="flex justify-between text-sm">
                    <span className="text-muted-foreground">Daily P&L</span>
                    <span className={riskStatus.circuit_breaker.daily_pnl < 0 ? 'text-red-500' : 'text-green-500'}>
                      {formatCurrency(riskStatus.circuit_breaker.daily_pnl, { showSign: true })}
                    </span>
                  </div>
                  <div className="flex justify-between text-sm">
                    <span className="text-muted-foreground">Consecutive Losses</span>
                    <span>{riskStatus.circuit_breaker.consecutive_losses}</span>
                  </div>
                  <div className="flex justify-between text-sm">
                    <span className="text-muted-foreground">Trips Today</span>
                    <span>{riskStatus.circuit_breaker.trips_today}</span>
                  </div>
                </div>

                {/* Save Button */}
                {canManageRisk && (
                  <Button
                    onClick={handleSaveRisk}
                    disabled={isSavingRisk || !riskDirty}
                    className="w-full"
                  >
                    {isSavingRisk ? (
                      <>
                        <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
                        Saving...
                      </>
                    ) : riskDirty ? (
                      <>
                        <Save className="mr-2 h-4 w-4" />
                        Save Risk Configuration
                      </>
                    ) : (
                      <>
                        <Check className="mr-2 h-4 w-4" />
                        Saved
                      </>
                    )}
                  </Button>
                )}
              </>
            )}
          </CardContent>
        </Card>
      )}

      {/* Appearance */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Palette className="h-5 w-5" />
            Appearance
          </CardTitle>
          <CardDescription>
            Customize the dashboard appearance
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="font-medium">Theme</p>
              <p className="text-sm text-muted-foreground">
                Choose your preferred theme
              </p>
            </div>
            <div className="flex flex-wrap gap-2">
              {themeButtons.map(({ value, label }) => (
                <Button
                  key={value}
                  variant={appearance.theme === value ? 'default' : 'outline'}
                  size="sm"
                  onClick={() => updateAppearance({ theme: value })}
                >
                  {label}
                </Button>
              ))}
            </div>
          </div>
        </CardContent>
      </Card>

      <ConnectWalletModal open={connectWalletOpen} onOpenChange={setConnectWalletOpen} />

      {/* Invite Member Dialog */}
      {currentWorkspace && (
        <InviteMemberDialog
          workspaceId={currentWorkspace.id}
          open={inviteDialogOpen}
          onOpenChange={setInviteDialogOpen}
        />
      )}
    </div>
  );
}
