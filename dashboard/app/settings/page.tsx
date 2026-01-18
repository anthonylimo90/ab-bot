'use client';

import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
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
} from 'lucide-react';
import { useAuthStore } from '@/stores/auth-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { InviteMemberDialog } from '@/components/workspace/InviteMemberDialog';
import { MemberList } from '@/components/workspace/MemberList';
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

  return (
    <div className="max-w-3xl mx-auto space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Settings</h1>
          <p className="text-muted-foreground">
            Manage your account and preferences
          </p>
        </div>
        <div className="flex items-center gap-2">
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
            <p className="font-medium mb-2">Connected Wallet</p>
            <p className="text-sm text-muted-foreground mb-4">
              No wallet connected
            </p>
            <Button onClick={() => setConnectWalletOpen(true)}>
              Connect Wallet
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* Team Management */}
      {currentWorkspace && (
        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
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
                    <div key={invite.id} className="flex items-center justify-between p-3 rounded-lg border">
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
              <div className="flex items-center justify-between p-4 rounded-lg border hover:bg-muted/50 transition-colors cursor-pointer">
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
          <div className="flex items-center justify-between">
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

          <div className="flex items-center justify-between">
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

          <div className="flex items-center justify-between">
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

          <div className="flex items-center justify-between">
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
          <div className="flex items-center justify-between">
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

          <div className="flex items-center justify-between">
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

          <div className="flex items-center justify-between">
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
          <div className="flex items-center justify-between">
            <div>
              <p className="font-medium">Theme</p>
              <p className="text-sm text-muted-foreground">
                Choose your preferred theme
              </p>
            </div>
            <div className="flex gap-2">
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

      {/* Connect Wallet Modal */}
      <Dialog open={connectWalletOpen} onOpenChange={setConnectWalletOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Connect Wallet</DialogTitle>
            <DialogDescription>
              Connect your wallet to enable live trading
            </DialogDescription>
          </DialogHeader>
          <div className="py-6 space-y-4">
            <p className="text-sm text-muted-foreground text-center">
              Wallet connection is not yet implemented. This feature will allow
              you to connect your Polygon wallet for live trading.
            </p>
            <div className="flex flex-col gap-2">
              <Button variant="outline" disabled className="w-full">
                MetaMask (Coming Soon)
              </Button>
              <Button variant="outline" disabled className="w-full">
                WalletConnect (Coming Soon)
              </Button>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConnectWalletOpen(false)}>
              Close
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

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
