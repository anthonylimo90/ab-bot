'use client';

import { useState, useEffect, useRef } from 'react';
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
import {
  clearWithdrawalIntent,
  isWithdrawalIntentSettled,
  withdrawalIntent,
} from '@/lib/withdrawal-intent';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { useTradingSwitches } from '@/hooks/useTradingSwitches';
import { InviteMemberDialog } from '@/components/workspace/InviteMemberDialog';
import { MemberList } from '@/components/workspace/MemberList';
import { ConnectWalletModal } from '@/components/wallet/ConnectWalletModal';
import { InfoTooltip } from '@/components/shared/InfoTooltip';
import { PageIntro } from '@/components/shared/PageIntro';
import { ConfirmAction } from '@/components/shared/ConfirmAction';
import { useWalletStore } from '@/stores/wallet-store';
import {
  useCreateWalletWithdrawalMutation,
  useWalletBalanceQuery,
  useWalletWithdrawalsQuery,
  useWithdrawalCapacityQuery,
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
  // Why the last trading save was refused, until the next save.
  const [tradingSaveError, setTradingSaveError] = useState<string | null>(null);

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
  const withdrawalIntentRef = useRef<{ fingerprint: string; id: string } | null>(null);
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
  const canManageWithdrawals = currentUserRole === 'owner' || currentUserRole === 'admin';
  // Workspace settings (wallet connection, trading switches) are the owner's.
  const canUpdateWorkspace = currentUserRole === 'owner';

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

  // Service status, and the switches the server's environment holds on.
  const { serviceStatus, forced: forcedOn } = useTradingSwitches();

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
  } = useWalletBalanceQuery(primaryConnectedWallet?.address ?? null);
  // A withdrawal draws on the wallet's USDC.e or its pUSD, less what is held
  // for withdrawals in flight and open buys: less than the balance above.
  const { data: withdrawalCapacity, isError: withdrawalCapacityFailed } =
    useWithdrawalCapacityQuery(primaryConnectedWallet?.address ?? null, canManageWithdrawals);
  const { data: recentWithdrawals = [], isLoading: withdrawalsLoading } =
    useWalletWithdrawalsQuery(8);
  const createWithdrawalMutation = useCreateWalletWithdrawalMutation();

  // Initialize the risk form from the stored config, and again only when that
  // config changes: the status poll (P&L, equity) must not reset an edit.
  const riskConfig = riskStatus?.circuit_breaker?.config;
  const riskConfigKey = riskConfig ? JSON.stringify(riskConfig) : null;
  useEffect(() => {
    if (riskConfigKey) {
      const cfg = JSON.parse(riskConfigKey) as NonNullable<typeof riskConfig>;
      setRiskMaxDailyLoss(cfg.max_daily_loss);
      setRiskMaxDrawdownPct(Math.round(cfg.max_drawdown_pct * 10000) / 100); // ratio -> percent display
      setRiskMaxConsecutiveLosses(cfg.max_consecutive_losses);
      setRiskCooldownMinutes(cfg.cooldown_minutes);
      setRiskEnabled(cfg.enabled);
    }
  }, [riskConfigKey]);

  // Initialize walletConnectProjectId from workspace
  useEffect(() => {
    if (currentWorkspace?.walletconnect_project_id) {
      setWalletConnectProjectId(currentWorkspace.walletconnect_project_id);
    }
  }, [currentWorkspace?.walletconnect_project_id]);

  // Initialize the trading config from the saved workspace, each field again
  // only when its saved value changes: a poll that reads the same workspace
  // keeps an unsaved edit.
  const workspaceId = currentWorkspace?.id;
  const savedRpcUrl = currentWorkspace?.polygon_rpc_url || '';
  const savedArbAutoExecute = currentWorkspace?.arb_auto_execute ?? false;
  const savedLiveTrading = currentWorkspace?.live_trading_enabled ?? false;
  const savedExitHandler = currentWorkspace?.exit_handler_enabled ?? false;
  useEffect(() => {
    setPolygonRpcUrl(savedRpcUrl);
  }, [workspaceId, savedRpcUrl]);
  useEffect(() => {
    setAlchemyApiKey(''); // Never pre-fill masked key
  }, [workspaceId]);
  useEffect(() => {
    setArbAutoExecute(savedArbAutoExecute);
    setArbAutoExecuteDirty(false);
  }, [workspaceId, savedArbAutoExecute]);
  useEffect(() => {
    setLiveTradingEnabled(savedLiveTrading);
    setLiveTradingEnabledDirty(false);
  }, [workspaceId, savedLiveTrading]);
  useEffect(() => {
    setExitHandlerEnabled(savedExitHandler);
    setExitHandlerEnabledDirty(false);
  }, [workspaceId, savedExitHandler]);

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
      // Stored as saved: a re-read that started before the save cannot
      // put the old ID back.
      setCurrentWorkspace(
        // Sent as entered: an empty ID clears it (undefined would be
        // dropped from the request and change nothing).
        await api.updateWorkspace(currentWorkspace.id, {
          walletconnect_project_id: walletConnectProjectId.trim(),
        }),
      );
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
      if (polygonRpcUrl.trim()) updates.polygon_rpc_url = polygonRpcUrl.trim();
      if (alchemyApiKey.trim()) updates.alchemy_api_key = alchemyApiKey.trim();
      if (Object.keys(updates).length === 0) {
        toast.warning('No changes to save', 'Trading settings already match the current workspace');
        return;
      }
      const updatedWorkspace = await api.updateWorkspace(currentWorkspace.id, updates);
      setCurrentWorkspace(updatedWorkspace);
      // Saved: the form shows what is stored (the RPC URL masked again).
      setPolygonRpcUrl(updatedWorkspace.polygon_rpc_url || '');
      setArbAutoExecute(updatedWorkspace.arb_auto_execute ?? false);
      setLiveTradingEnabled(updatedWorkspace.live_trading_enabled ?? false);
      setExitHandlerEnabled(updatedWorkspace.exit_handler_enabled ?? false);
      setArbAutoExecuteDirty(false);
      setLiveTradingEnabledDirty(false);
      setExitHandlerEnabledDirty(false);
      setTradingSaveError(null);
      toast.success('Trading config saved', 'Your trading configuration has been updated');
      queryClient.invalidateQueries({ queryKey: ['workspace'] });
      queryClient.invalidateQueries({ queryKey: queryKeys.runtime.serviceStatus() });
      setAlchemyApiKey(''); // Clear after save
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Unknown error';
      // A refused save changed nothing: show the switches as they are saved
      // (read again, in case the save did land), not as they were asked for.
      const saved = await api.getWorkspace(currentWorkspace.id).catch(() => null);
      if (saved) setCurrentWorkspace(saved);
      const source = saved ?? currentWorkspace;
      setArbAutoExecute(source.arb_auto_execute ?? false);
      setLiveTradingEnabled(source.live_trading_enabled ?? false);
      setExitHandlerEnabled(source.exit_handler_enabled ?? false);
      setArbAutoExecuteDirty(false);
      setLiveTradingEnabledDirty(false);
      setExitHandlerEnabledDirty(false);
      setTradingSaveError(message);
      toast.error('Failed to save', message);
    } finally {
      setIsSavingTradingConfig(false);
    }
  };

  // Save risk management config (real API call). Only the fields changed
  // from the stored config are sent: the others keep whatever is stored, even
  // if another operator changed them meanwhile, and the drawdown limit is not
  // re-sent rounded to the two decimals the form shows.
  const handleSaveRisk = async () => {
    if (!currentWorkspace || !riskConfig) return;
    const changed = <T,>(value: T | '', stored: T) =>
      value === '' || value === stored ? undefined : value;
    const shownDrawdown = Math.round(riskConfig.max_drawdown_pct * 10000) / 100;
    setIsSavingRisk(true);
    try {
      const drawdownPct = changed(riskMaxDrawdownPct, shownDrawdown);
      await api.updateCircuitBreakerConfig(currentWorkspace.id, {
        max_daily_loss: changed(riskMaxDailyLoss, riskConfig.max_daily_loss),
        max_drawdown_pct: drawdownPct === undefined ? undefined : drawdownPct / 100, // percent -> ratio
        max_consecutive_losses: changed(
          riskMaxConsecutiveLosses,
          riskConfig.max_consecutive_losses,
        ),
        cooldown_minutes: changed(riskCooldownMinutes, riskConfig.cooldown_minutes),
        enabled: riskEnabled === riskConfig.enabled ? undefined : riskEnabled,
      });
      toast.success('Risk settings saved', 'Circuit breaker configuration has been updated');
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.status() });
    } catch (err) {
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.status() });
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
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.status() });
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
      queryClient.invalidateQueries({ queryKey: queryKeys.risk.status() });
      toast.error('Failed to reset circuit breaker', err instanceof Error ? err.message : 'Unknown error');
    } finally {
      setIsResettingCb(false);
    }
  };

  // Turning live trading or automatic arbitrage on is confirmed first: with
  // both on, the bot spends real funds on its own.
  const enablingLiveTrading =
    liveTradingEnabledDirty && liveTradingEnabled && !currentWorkspace?.live_trading_enabled;
  const enablingArbAutoExecute =
    arbAutoExecuteDirty && arbAutoExecute && !currentWorkspace?.arb_auto_execute;
  // Switching the exit handler off is confirmed too: open positions stop
  // being sold, and live entries stop with it.
  const disablingExitHandler =
    exitHandlerEnabledDirty && !exitHandlerEnabled && Boolean(currentWorkspace?.exit_handler_enabled);
  const tradingSaveConfirm =
    enablingLiveTrading || enablingArbAutoExecute
      ? {
          title:
            enablingLiveTrading && enablingArbAutoExecute
              ? 'Turn on live trading and automatic arbitrage?'
              : enablingLiveTrading
                ? 'Turn on live trading?'
                : 'Turn on automatic arbitrage?',
          description:
            (enablingLiveTrading
              ? 'Orders will use real funds from the active wallet, including the automatic strategies that are on. Check the wallet and its balance first.'
              : 'The bot will place arbitrage trades on its own; with live trading on, they use real funds from the active wallet.') +
            (disablingExitHandler
              ? ' The exit handler is also switched off, so open positions are not sold automatically and live entries do not start.'
              : ''),
          confirmLabel: 'Save and Turn On',
        }
      : disablingExitHandler
        ? {
            title: 'Switch the exit handler off?',
            description:
              'Open positions are no longer sold automatically: exit conditions and queued exits wait until it is back on. New live entries stop too, since a live entry needs the exit handler running. Resolved markets are still booked.',
            confirmLabel: 'Save and Switch Off',
          }
        : null;
  const tradingSaveButton = (onClick?: () => void) => (
    <Button onClick={onClick} disabled={isSavingTradingConfig} className="w-full">
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
  );

  // The drawdown limit must stay below the hard kill (the server refuses
  // otherwise): a limit at or past it would never trip first.
  const hardKillPct =
    riskConfig?.hard_kill_drawdown_pct != null
      ? Math.round(riskConfig.hard_kill_drawdown_pct * 10000) / 100
      : null;
  // Only a changed limit is sent (and checked by the server): a stored one
  // past the hard kill must not block saving the other fields.
  const storedDrawdownPct =
    riskConfig != null ? Math.round(riskConfig.max_drawdown_pct * 10000) / 100 : null;
  const drawdownAtHardKill =
    hardKillPct != null &&
    hardKillPct > 0 &&
    riskMaxDrawdownPct !== '' &&
    riskMaxDrawdownPct !== storedDrawdownPct &&
    riskMaxDrawdownPct >= hardKillPct;

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
      toast.success('Primary wallet updated', 'This wallet is now selected for balances and withdrawals.');
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
    const sourceAddress = primaryConnectedWallet?.address;
    if (!sourceAddress) {
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

    if (withdrawDestination.trim().toLowerCase() === sourceAddress.toLowerCase()) {
      toast.error(
        'Same wallet selected',
        'The destination matches the source wallet. If this is already your MetaMask address, no transfer is needed.',
      );
      return;
    }

    try {
      const fingerprint = JSON.stringify([user?.id, sourceAddress.toLowerCase(), withdrawDestination.trim().toLowerCase(), amount]);
      const intent = withdrawalIntent(sessionStorage, fingerprint, withdrawalIntentRef.current);
      withdrawalIntentRef.current = intent;
      const result = await createWithdrawalMutation.mutateAsync({
        client_request_id: intent.id,
        source_address: sourceAddress,
        destination_address: withdrawDestination.trim(),
        amount,
      });
      // Confirmed or terminally failed ends this intent so the next attempt gets a
      // new request id; pending/submitted/unknown keep it so a retry stays idempotent.
      if (isWithdrawalIntentSettled(result.status)) {
        clearWithdrawalIntent(sessionStorage);
        withdrawalIntentRef.current = null;
      }
      if (result.status === 'confirmed') {
        setWithdrawDestination('');
        setWithdrawAmount('');
      }
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
      <Card id="wallet-account" className="scroll-mt-24">
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Wallet className="h-5 w-5" />
            Account
          </CardTitle>
          <CardDescription>
            Manage your personal wallets and withdraw settled USDC when needed
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="rounded-lg border p-4">
            <div className="mb-4 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="flex items-center gap-2 font-medium">
                  <span>Connected Wallets</span>
                  <InfoTooltip content="Your primary wallet is used for balances and withdrawals. Changing it does not switch the bot's trading wallet." />
                </p>
                <p className="text-sm text-muted-foreground">
                  Your primary wallet is used for balances and withdrawals
                </p>
                <p className="mt-1 text-xs text-muted-foreground break-all">
                  Bot trading wallet: {currentWorkspace?.trading_wallet_address || 'Not configured'}
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
                            Primary
                          </span>
                        ) : (
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={() => handleMakePrimary(wallet.address)}
                            disabled={walletLoading}
                          >
                            Make Primary
                          </Button>
                        )}
                        <ConfirmAction
                          title="Disconnect this wallet?"
                          description={`${wallet.address} is removed from your account and its private key is erased from the vault. Funds stay in the wallet; reach them only with your own copy of the key.${
                            wallet.address.toLowerCase() ===
                            currentWorkspace?.trading_wallet_address?.toLowerCase()
                              ? " This is the bot's trading wallet: once its key is erased, live trading cannot sign orders or withdrawals from it until the key is added again."
                              : ''
                          }`}
                          confirmLabel="Disconnect and Erase Key"
                          destructive
                          onConfirm={() => handleDisconnectWallet(wallet.address)}
                        >
                          <Button
                            variant="ghost"
                            size="sm"
                            disabled={walletLoading}
                            aria-label={`Disconnect wallet ${wallet.label || shortAddress(wallet.address)}`}
                          >
                            <Trash2 className="h-4 w-4" />
                          </Button>
                        </ConfirmAction>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          <div id="withdraw-usdc" className="rounded-lg border p-4 scroll-mt-24">
            <div className="mb-4">
              <p className="flex items-center gap-2 font-medium">
                <span>Withdraw USDC</span>
                <InfoTooltip content="This sends real USDC.e on Polygon from your primary wallet: its USDC.e, or its pUSD trading collateral unwrapped to the recipient. It only moves free USDC, not open position inventory." />
              </p>
              <p className="text-sm text-muted-foreground">
                Send settled USDC from your primary wallet to another Polygon address, such as a different MetaMask wallet.
              </p>
            </div>

            {!canManageWithdrawals ? (
              <p className="text-sm text-muted-foreground">
                Only workspace owners and admins can initiate withdrawals.
              </p>
            ) : walletLoading && connectedWallets.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                Loading primary wallet details...
              </p>
            ) : !primaryConnectedWallet ? (
              <p className="text-sm text-muted-foreground">
                Connect a wallet and make it primary before withdrawing funds.
              </p>
            ) : (
              <div className="space-y-4">
                <div className="rounded-md border bg-muted/30 p-3 text-sm">
                  <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
                    <div>
                      <p className="font-medium">
                        Source wallet: {primaryConnectedWallet.label || shortAddress(primaryConnectedWallet.address)}
                      </p>
                      <p className="font-mono text-xs text-muted-foreground">
                        {primaryConnectedWallet.address}
                      </p>
                    </div>
                    <div className="text-sm font-medium">
                      {isPrimaryWalletBalancePending
                        ? 'Loading balance...'
                        : primaryWalletBalance
                          ? `Wallet cash ${formatCurrency(primaryWalletBalance.usdc_balance)}`
                          : 'Balance unavailable'}
                    </div>
                  </div>
                  <p className="mt-2 text-xs text-muted-foreground">
                    {withdrawalCapacity
                      ? `Up to ${formatCurrency(withdrawalCapacity.max_withdrawable)} can be withdrawn in one transfer now.`
                      : withdrawalCapacityFailed
                        ? 'The amount that can be withdrawn now could not be checked.'
                        : 'Checking how much can be withdrawn now...'}
                  </p>
                  {withdrawalCapacity && withdrawalCapacity.blocking_reasons.length > 0 && (
                    <ul className="mt-1 list-disc pl-5 text-xs text-destructive">
                      {withdrawalCapacity.blocking_reasons.map((reason) => (
                        <li key={reason}>{reason}</li>
                      ))}
                    </ul>
                  )}
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
                  <ConfirmAction
                    title="Send this withdrawal?"
                    description={`${Number(withdrawAmount).toLocaleString('en-US', { maximumFractionDigits: 6 })} USDC to ${withdrawDestination.trim()}. A transfer on Polygon cannot be undone: check the address.`}
                    confirmLabel="Send Withdrawal"
                    onConfirm={handleWithdraw}
                  >
                    <Button
                      disabled={
                        createWithdrawalMutation.isPending ||
                        !withdrawDestination ||
                        !withdrawAmount ||
                        withdrawalCapacity?.can_withdraw === false
                      }
                      title={
                        withdrawalCapacity?.can_withdraw === false
                          ? withdrawalCapacity.blocking_reasons.join('; ') ||
                            'A withdrawal cannot be sent now'
                          : undefined
                      }
                      className="w-full sm:w-auto"
                    >
                      {createWithdrawalMutation.isPending ? (
                        <RefreshCw className="h-4 w-4 animate-spin" />
                      ) : (
                        <ArrowUpRight className="h-4 w-4" />
                      )}
                      <span className="ml-2">Withdraw</span>
                    </Button>
                  </ConfirmAction>
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

      {/* WalletConnect Settings (Owner only) */}
      {currentWorkspace && canUpdateWorkspace && (
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

      {/* Trading Configuration (Owner only) */}
      {currentWorkspace && canUpdateWorkspace && (
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
                {(
                  [
                    ["Harvester", serviceStatus.harvester],
                    ["Metrics calculator", serviceStatus.metrics_calculator],
                    ["Arb executor", serviceStatus.arb_executor],
                    ["Exit handler", serviceStatus.exit_handler],
                    ["Live trading", serviceStatus.live_trading],
                  ] as const
                ).map(([name, status]) => (
                  <div key={name} className="flex flex-col gap-1 text-sm sm:flex-row sm:items-center sm:justify-between">
                    <span>{name}</span>
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
                {serviceStatus.strategy_modes.length > 0 && (
                  <div className="space-y-1 border-t pt-3">
                    <p className="text-xs font-medium text-muted-foreground">Strategy modes</p>
                    {serviceStatus.strategy_modes.map((strategy) => (
                      <div
                        key={strategy.strategy}
                        className="flex flex-col gap-1 text-sm sm:flex-row sm:items-center sm:justify-between"
                      >
                        <span>{strategy.strategy.replace(/_/g, ' ')}</span>
                        <span
                          className={
                            strategy.mode === 'live'
                              ? 'text-green-600'
                              : 'text-muted-foreground'
                          }
                        >
                          {strategy.mode}
                          {strategy.reason ? ` — ${strategy.reason}` : ''}
                        </span>
                      </div>
                    ))}
                  </div>
                )}
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
                checked={arbAutoExecute || forcedOn.arb_auto_execute}
                disabled={forcedOn.arb_auto_execute}
                onCheckedChange={(checked) => {
                  setArbAutoExecute(checked);
                  setArbAutoExecuteDirty(true);
                }}
              />
            </div>
            {forcedOn.arb_auto_execute && (
              <p className="text-xs text-muted-foreground">
                On: ARB_AUTO_EXECUTE=true in the server&apos;s environment holds it on.
              </p>
            )}

            {/* Exit Handler Toggle */}
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="font-medium">Exit Handler</p>
                <p className="text-sm text-muted-foreground">
                  Let the bot close positions automatically when an exit condition or market resolution is reached
                </p>
              </div>
              <Switch
                checked={exitHandlerEnabled || forcedOn.exit_handler_enabled}
                disabled={forcedOn.exit_handler_enabled}
                onCheckedChange={(checked) => {
                  setExitHandlerEnabled(checked);
                  setExitHandlerEnabledDirty(true);
                }}
              />
            </div>
            {forcedOn.exit_handler_enabled && (
              <p className="text-xs text-muted-foreground">
                On: EXIT_HANDLER_ENABLED=true in the server&apos;s environment holds it on.
              </p>
            )}

            {/* Live Trading Toggle */}
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="font-medium">Live Trading</p>
                <p className="text-sm text-muted-foreground">
                  Allow the system to place real trades with the active wallet
                </p>
              </div>
              <Switch
                checked={liveTradingEnabled || forcedOn.live_trading_enabled}
                disabled={forcedOn.live_trading_enabled}
                onCheckedChange={(checked) => {
                  setLiveTradingEnabled(checked);
                  setLiveTradingEnabledDirty(true);
                }}
              />
            </div>
            {forcedOn.live_trading_enabled && (
              <p className="text-xs text-muted-foreground">
                On: LIVE_TRADING=true in the server&apos;s environment holds it on.
              </p>
            )}
            {(liveTradingEnabled || forcedOn.live_trading_enabled) && (
              <div className="rounded-lg border border-yellow-500/20 bg-yellow-500/10 p-3">
                <p className="text-sm text-yellow-500 flex items-center gap-2">
                  <AlertTriangle className="h-4 w-4" />
                  Live trading uses real funds. Only turn this on when the active wallet is correct and funded.
                </p>
              </div>
            )}

            {tradingSaveError && (
              <p role="alert" className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
                <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
                <span>Not saved: {tradingSaveError}</span>
              </p>
            )}
            {tradingSaveConfirm ? (
              <ConfirmAction
                {...tradingSaveConfirm}
                destructive
                onConfirm={handleSaveTradingConfig}
              >
                {tradingSaveButton()}
              </ConfirmAction>
            ) : (
              tradingSaveButton(handleSaveTradingConfig)
            )}
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
                      {hardKillPct != null && hardKillPct > 0 && (
                        <> (below the {hardKillPct}% hard kill, which halts until reset)</>
                      )}
                    </p>
                    {drawdownAtHardKill && (
                      <p className="text-xs text-destructive">
                        Must be below the {hardKillPct}% hard kill: at or past it, the hard kill would trip first.
                      </p>
                    )}
                  </div>
                  <div className="flex items-center gap-2">
                    <input
                      type="number"
                      value={riskMaxDrawdownPct}
                      onChange={(e) => setRiskMaxDrawdownPct(e.target.value === '' ? '' : Number(e.target.value))}
                      className="w-20 rounded border bg-background px-3 py-1 text-right"
                      min={1}
                      max={hardKillPct != null && hardKillPct > 0 ? hardKillPct : 100}
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
                      min={1}
                      max={10080}
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
                      <ConfirmAction
                        title="Trip the circuit breaker?"
                        description="All automated trading stops at once and stays stopped until the breaker is reset."
                        confirmLabel="Trip Circuit Breaker"
                        destructive
                        onConfirm={handleTripCircuitBreaker}
                      >
                      <Button
                        variant="destructive"
                        size="sm"
                        disabled={isTrippingCb || riskStatus?.circuit_breaker?.tripped}
                      >
                        {isTrippingCb ? (
                          <><RefreshCw className="mr-1 h-3 w-3 animate-spin" />Tripping...</>
                        ) : (
                          <><AlertTriangle className="mr-1 h-3 w-3" />Trip</>
                        )}
                      </Button>
                      </ConfirmAction>
                      <ConfirmAction
                        title="Reset the circuit breaker?"
                        description="Trading resumes at once, ending the current trip and its cooldown. Reset only once its cause is understood."
                        confirmLabel="Reset and Resume Trading"
                        onConfirm={handleResetCircuitBreaker}
                      >
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={isResettingCb || !riskStatus?.circuit_breaker?.tripped}
                      >
                        {isResettingCb ? (
                          <><RefreshCw className="mr-1 h-3 w-3 animate-spin" />Resetting...</>
                        ) : (
                          <><Shield className="mr-1 h-3 w-3" />Reset</>
                        )}
                      </Button>
                      </ConfirmAction>
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

                {/* Save Button: switching the breaker off asks first */}
                {canManageRisk &&
                  (!riskEnabled && riskStatus?.circuit_breaker?.config?.enabled ? (
                    <ConfirmAction
                      title="Switch the circuit breaker off?"
                      description="Its loss limits stop guarding trading until it is switched back on. A latched trip (manual or hard kill) still holds until it is reset."
                      confirmLabel="Save and Switch Off"
                      destructive
                      onConfirm={handleSaveRisk}
                    >
                  <Button
                    disabled={isSavingRisk || !riskDirty || drawdownAtHardKill}
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
                    </ConfirmAction>
                  ) : (
                  <Button
                    onClick={handleSaveRisk}
                    disabled={isSavingRisk || !riskDirty || drawdownAtHardKill}
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
                  ))}
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
