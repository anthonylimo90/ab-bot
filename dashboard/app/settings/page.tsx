'use client';

import { useState, useEffect } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { useSettingsStore, Theme } from '@/stores/settings-store';
import { useToastStore } from '@/stores/toast-store';
import {
  RefreshCw,
  Wallet,
  Bell,
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
} from 'lucide-react';
import { useAuthStore } from '@/stores/auth-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { InviteMemberDialog } from '@/components/workspace/InviteMemberDialog';
import { MemberList } from '@/components/workspace/MemberList';
import { ConnectWalletModal } from '@/components/wallet/ConnectWalletModal';
import { useWalletStore } from '@/stores/wallet-store';
import api from '@/lib/api';
import Link from 'next/link';

export default function SettingsPage() {
  const toast = useToastStore();
  const queryClient = useQueryClient();
  const { user } = useAuthStore();
  const { currentWorkspace } = useWorkspaceStore();
  const {
    risk,
    notifications,
    appearance,
    isDirty,
    updateRisk,
    updateNotifications,
    updateAppearance,
    markClean,
    resetToDefaults,
  } = useSettingsStore();

  const [connectWalletOpen, setConnectWalletOpen] = useState(false);
  const [inviteDialogOpen, setInviteDialogOpen] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [walletConnectProjectId, setWalletConnectProjectId] = useState('');
  const [isSavingWalletConnect, setIsSavingWalletConnect] = useState(false);
  const [isSavingTradingConfig, setIsSavingTradingConfig] = useState(false);
  const [polygonRpcUrl, setPolygonRpcUrl] = useState('');
  const [alchemyApiKey, setAlchemyApiKey] = useState('');
  const [copyTradingEnabled, setCopyTradingEnabled] = useState(false);
  const [arbAutoExecute, setArbAutoExecute] = useState(false);
  const [liveTradingEnabled, setLiveTradingEnabled] = useState(false);
  const {
    connectedWallets,
    primaryWallet,
    isLoading: walletLoading,
    fetchWallets,
    setPrimary,
    disconnectWallet,
  } = useWalletStore();

  // Fetch workspace members
  const { data: members = [], isLoading: membersLoading } = useQuery({
    queryKey: ['workspace', currentWorkspace?.id, 'members'],
    queryFn: () => api.listWorkspaceMembers(currentWorkspace!.id),
    enabled: !!currentWorkspace?.id,
  });

  // Fetch pending invites
  const { data: invites = [], isLoading: invitesLoading } = useQuery({
    queryKey: ['workspace', currentWorkspace?.id, 'invites'],
    queryFn: () => api.listWorkspaceInvites(currentWorkspace!.id),
    enabled: !!currentWorkspace?.id,
  });

  // Get current user's role in workspace
  const currentUserRole = currentWorkspace?.my_role;
  const canInvite = currentUserRole === 'owner' || currentUserRole === 'admin';
  const canConfigureWalletConnect = currentUserRole === 'owner' || currentUserRole === 'admin';

  // Fetch service status
  const { data: serviceStatus } = useQuery({
    queryKey: ['workspace', currentWorkspace?.id, 'service-status'],
    queryFn: () => api.getServiceStatus(currentWorkspace!.id),
    enabled: !!currentWorkspace?.id && canConfigureWalletConnect,
    refetchInterval: 30000, // refresh every 30s
  });

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
      setCopyTradingEnabled(currentWorkspace.copy_trading_enabled ?? false);
      setArbAutoExecute(currentWorkspace.arb_auto_execute ?? false);
      setLiveTradingEnabled(currentWorkspace.live_trading_enabled ?? false);
    }
  }, [currentWorkspace]);

  useEffect(() => {
    fetchWallets().catch(() => {
      // Wallet card will show empty/fallback state if loading fails.
    });
  }, [fetchWallets]);

  // Revoke invite mutation
  const revokeInviteMutation = useMutation({
    mutationFn: (inviteId: string) => api.revokeInvite(currentWorkspace!.id, inviteId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['workspace', currentWorkspace?.id, 'invites'] });
      toast.success('Invite revoked', 'The invitation has been cancelled');
    },
    onError: (err: Error) => {
      toast.error('Failed to revoke invite', err.message);
    },
  });

  // Save WalletConnect project ID
  const handleSaveWalletConnect = async () => {
    if (!currentWorkspace) return;
    setIsSavingWalletConnect(true);
    try {
      await api.updateWorkspace(currentWorkspace.id, {
        walletconnect_project_id: walletConnectProjectId || undefined,
      });
      toast.success('WalletConnect settings saved', 'Your wallet connection is now configured');
      // Refresh workspace to get updated config
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
      const updates: Record<string, unknown> = {
        copy_trading_enabled: copyTradingEnabled,
        arb_auto_execute: arbAutoExecute,
        live_trading_enabled: liveTradingEnabled,
      };
      if (polygonRpcUrl) updates.polygon_rpc_url = polygonRpcUrl;
      if (alchemyApiKey) updates.alchemy_api_key = alchemyApiKey;
      await api.updateWorkspace(currentWorkspace.id, updates);
      toast.success('Trading config saved', 'Your trading configuration has been updated');
      queryClient.invalidateQueries({ queryKey: ['workspace'] });
      queryClient.invalidateQueries({ queryKey: ['workspace', currentWorkspace.id, 'service-status'] });
      setAlchemyApiKey(''); // Clear after save
    } catch (err) {
      toast.error('Failed to save', err instanceof Error ? err.message : 'Unknown error');
    } finally {
      setIsSavingTradingConfig(false);
    }
  };

  const handleSave = async () => {
    setIsSaving(true);
    // Simulate API call
    await new Promise((resolve) => setTimeout(resolve, 600));
    markClean();
    toast.success('Settings saved', 'Your preferences have been updated');
    setIsSaving(false);
  };

  const handleReset = () => {
    resetToDefaults();
    toast.info('Settings reset', 'All settings have been restored to defaults');
  };

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

  const shortAddress = (address: string) => `${address.slice(0, 6)}...${address.slice(-4)}`;

  return (
    <div className="mx-auto max-w-3xl space-y-5 sm:space-y-6">
      {/* Page Header */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h1 className="text-2xl font-bold tracking-tight sm:text-3xl">Settings</h1>
          <p className="text-muted-foreground">
            Manage your account and preferences
          </p>
        </div>
        <div className="flex w-full flex-wrap items-center gap-2 sm:w-auto sm:justify-end">
          {isDirty && (
            <span className="text-sm text-yellow-500 flex items-center gap-1">
              <AlertTriangle className="h-4 w-4" />
              Unsaved changes
            </span>
          )}
          <Button
            variant="outline"
            onClick={handleReset}
            disabled={isSaving}
          >
            Reset to Defaults
          </Button>
          <Button
            onClick={handleSave}
            disabled={!isDirty || isSaving}
          >
            {isSaving ? (
              <>
                <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
                Saving...
              </>
            ) : isDirty ? (
              <>
                <Save className="mr-2 h-4 w-4" />
                Save Changes
              </>
            ) : (
              <>
                <Check className="mr-2 h-4 w-4" />
                Saved
              </>
            )}
          </Button>
        </div>
      </div>

      {/* Account */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Wallet className="h-5 w-5" />
            Account
          </CardTitle>
          <CardDescription>
            Wallet configuration for live trading
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="rounded-lg border p-4">
            <div className="mb-4 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="font-medium">Connected Wallets</p>
                <p className="text-sm text-muted-foreground">
                  Primary wallet is used automatically for live trading
                </p>
              </div>
              <Button onClick={() => setConnectWalletOpen(true)} className="w-full sm:w-auto">Connect Wallet</Button>
            </div>

            {walletLoading && connectedWallets.length === 0 ? (
              <p className="text-sm text-muted-foreground">Loading wallets...</p>
            ) : connectedWallets.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No wallets connected yet. Connect one to enable live trading.
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
                            Set Active
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
              Configure WalletConnect for MetaMask and other wallet connections
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2">
              <label className="text-sm font-medium" htmlFor="walletconnect-project-id">
                WalletConnect Project ID
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
                  Wallet connection requires a WalletConnect project ID. Create a free account at WalletConnect Cloud to get started.
                </p>
              </div>
            )}

            {(walletConnectProjectId || currentWorkspace.walletconnect_project_id) && (
              <div className="rounded-lg border border-green-500/20 bg-green-500/10 p-3">
                <p className="text-sm text-green-500 flex items-center gap-2">
                  <Check className="h-4 w-4" />
                  Wallet connection is configured. Users can connect their MetaMask wallets.
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
              Configure trading services and blockchain connectivity
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            {/* Service Status Indicators */}
            {serviceStatus && (
              <div className="rounded-lg border p-4 space-y-2">
                <p className="text-sm font-medium mb-3">Service Status</p>
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
              <label className="text-sm font-medium" htmlFor="polygon-rpc-url">
                Polygon RPC URL
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
                Required for copy trading and wallet monitoring. Accepted providers:
                Alchemy, Infura, Ankr, Polygon, LlamaRPC, DRPC, PublicNode, 1RPC,
                Tenderly, Particle Network.
              </p>
            </div>

            {/* Alchemy API Key */}
            <div className="space-y-2">
              <label className="text-sm font-medium" htmlFor="alchemy-api-key">
                Alchemy API Key
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
                Alternative to Polygon RPC URL. Leave empty to keep current key.
              </p>
            </div>

            {/* Copy Trading Toggle */}
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="font-medium">Copy Trading</p>
                <p className="text-sm text-muted-foreground">
                  Automatically replicate trades from tracked wallets
                </p>
              </div>
              <Switch
                checked={copyTradingEnabled}
                onCheckedChange={setCopyTradingEnabled}
              />
            </div>

            {/* Arb Auto-Execute Toggle */}
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="font-medium">Arbitrage Auto-Execute</p>
                <p className="text-sm text-muted-foreground">
                  Automatically execute detected arbitrage opportunities
                </p>
              </div>
              <Switch
                checked={arbAutoExecute}
                onCheckedChange={setArbAutoExecute}
              />
            </div>

            {/* Live Trading Toggle */}
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="font-medium">Live Trading</p>
                <p className="text-sm text-muted-foreground">
                  Execute real orders with connected wallet
                </p>
              </div>
              <Switch
                checked={liveTradingEnabled}
                onCheckedChange={setLiveTradingEnabled}
              />
            </div>
            {liveTradingEnabled && (
              <div className="rounded-lg border border-yellow-500/20 bg-yellow-500/10 p-3">
                <p className="text-sm text-yellow-500 flex items-center gap-2">
                  <AlertTriangle className="h-4 w-4" />
                  Live trading uses real funds. Ensure a wallet is connected and funded.
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
                  Save Trading Configuration
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
                  Manage workspace members and invitations
                </CardDescription>
              </div>
              {canInvite && (
                <Button onClick={() => setInviteDialogOpen(true)}>
                  <UserPlus className="mr-2 h-4 w-4" />
                  Invite Member
                </Button>
              )}
            </div>
          </CardHeader>
          <CardContent className="space-y-6">
            {/* Members List */}
            <div>
              <h3 className="text-sm font-medium mb-3">Members ({members.length})</h3>
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
                        onClick={() => revokeInviteMutation.mutate(invite.id)}
                        disabled={revokeInviteMutation.isPending}
                      >
                        {revokeInviteMutation.isPending ? 'Revoking...' : 'Revoke'}
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
              Manage user accounts and permissions
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Link href="/settings/users">
              <div className="flex items-center justify-between rounded-lg border p-4 transition-colors hover:bg-muted/50 cursor-pointer">
                <div>
                  <p className="font-medium">Manage Users</p>
                  <p className="text-sm text-muted-foreground">
                    Create, edit, and delete user accounts
                  </p>
                </div>
                <ChevronRight className="h-5 w-5 text-muted-foreground" />
              </div>
            </Link>
          </CardContent>
        </Card>
      )}

      {/* Risk Management */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Shield className="h-5 w-5" />
            Risk Management
          </CardTitle>
          <CardDescription>
            Configure risk parameters for your trades
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="font-medium">Default Stop-Loss</p>
              <p className="text-sm text-muted-foreground">
                Automatically set stop-loss on new positions
              </p>
            </div>
            <div className="flex items-center gap-2">
              <input
                type="number"
                value={risk.defaultStopLoss}
                onChange={(e) =>
                  updateRisk({ defaultStopLoss: Number(e.target.value) })
                }
                className="w-20 rounded border bg-background px-3 py-1 text-right"
                min={1}
                max={50}
              />
              <span className="text-muted-foreground">%</span>
            </div>
          </div>

          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="font-medium">Max Position Size</p>
              <p className="text-sm text-muted-foreground">
                Maximum amount per single position
              </p>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-muted-foreground">$</span>
              <input
                type="number"
                value={risk.maxPositionSize}
                onChange={(e) =>
                  updateRisk({ maxPositionSize: Number(e.target.value) })
                }
                className="w-24 rounded border bg-background px-3 py-1 text-right"
                min={10}
                max={10000}
              />
            </div>
          </div>

          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="font-medium">Daily Loss Limit</p>
              <p className="text-sm text-muted-foreground">
                Maximum daily loss before circuit breaker triggers
              </p>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-muted-foreground">$</span>
              <input
                type="number"
                value={risk.dailyLossLimit}
                onChange={(e) =>
                  updateRisk({ dailyLossLimit: Number(e.target.value) })
                }
                className="w-24 rounded border bg-background px-3 py-1 text-right"
                min={100}
                max={50000}
              />
            </div>
          </div>

          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="font-medium">Circuit Breaker</p>
              <p className="text-sm text-muted-foreground">
                Pause trading after daily loss exceeds threshold
              </p>
            </div>
            <Switch
              checked={risk.circuitBreakerEnabled}
              onCheckedChange={(checked) =>
                updateRisk({ circuitBreakerEnabled: checked })
              }
            />
          </div>
        </CardContent>
      </Card>

      {/* Notifications */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Bell className="h-5 w-5" />
            Notifications
          </CardTitle>
          <CardDescription>
            Configure alerts and notifications
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="font-medium">Telegram Alerts</p>
              <p className="text-sm text-muted-foreground">
                Receive trade notifications via Telegram
              </p>
            </div>
            <div className="flex items-center gap-2">
              <Switch
                checked={notifications.telegramEnabled}
                onCheckedChange={(checked) =>
                  updateNotifications({ telegramEnabled: checked })
                }
              />
              {notifications.telegramEnabled && (
                <Button variant="outline" size="sm">
                  Configure
                </Button>
              )}
            </div>
          </div>

          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="font-medium">Discord Webhook</p>
              <p className="text-sm text-muted-foreground">
                Post updates to a Discord channel
              </p>
            </div>
            <div className="flex items-center gap-2">
              <Switch
                checked={notifications.discordEnabled}
                onCheckedChange={(checked) =>
                  updateNotifications({ discordEnabled: checked })
                }
              />
              {notifications.discordEnabled && (
                <Button variant="outline" size="sm">
                  Configure
                </Button>
              )}
            </div>
          </div>

          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="font-medium">Email Notifications</p>
              <p className="text-sm text-muted-foreground">
                Daily summary and important alerts
              </p>
            </div>
            <Switch
              checked={notifications.emailEnabled}
              onCheckedChange={(checked) =>
                updateNotifications({ emailEnabled: checked })
              }
            />
          </div>
        </CardContent>
      </Card>

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
