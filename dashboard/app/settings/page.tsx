'use client';

import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { useModeStore } from '@/stores/mode-store';
import { formatCurrency } from '@/lib/utils';
import { RefreshCw, Wallet, Bell, Shield, Palette } from 'lucide-react';

export default function SettingsPage() {
  const { mode, setMode, demoBalance, resetDemoBalance } = useModeStore();
  const isDemo = mode === 'demo';

  return (
    <div className="max-w-3xl mx-auto space-y-6">
      {/* Page Header */}
      <div>
        <h1 className="text-3xl font-bold tracking-tight">Settings</h1>
        <p className="text-muted-foreground">
          Manage your account and preferences
        </p>
      </div>

      {/* Account */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Wallet className="h-5 w-5" />
            Account
          </CardTitle>
          <CardDescription>
            Trading mode and wallet configuration
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="flex items-center justify-between">
            <div>
              <p className="font-medium">Trading Mode</p>
              <p className="text-sm text-muted-foreground">
                {isDemo
                  ? 'Paper trading with simulated funds'
                  : 'Live trading with real funds'}
              </p>
            </div>
            <div className="flex items-center gap-3">
              <span className={isDemo ? 'text-demo' : 'text-muted-foreground'}>
                Demo
              </span>
              <Switch
                checked={!isDemo}
                onCheckedChange={(checked) => setMode(checked ? 'live' : 'demo')}
              />
              <span className={!isDemo ? 'text-live' : 'text-muted-foreground'}>
                Live
              </span>
            </div>
          </div>

          {isDemo && (
            <div className="rounded-lg border p-4 bg-muted/30">
              <div className="flex items-center justify-between">
                <div>
                  <p className="font-medium">Demo Balance</p>
                  <p className="text-2xl font-bold tabular-nums">
                    {formatCurrency(demoBalance)}
                  </p>
                </div>
                <Button variant="outline" onClick={resetDemoBalance}>
                  <RefreshCw className="mr-2 h-4 w-4" />
                  Reset Balance
                </Button>
              </div>
            </div>
          )}

          {!isDemo && (
            <div className="rounded-lg border p-4">
              <p className="font-medium mb-2">Connected Wallet</p>
              <p className="text-sm text-muted-foreground mb-4">
                No wallet connected
              </p>
              <Button>Connect Wallet</Button>
            </div>
          )}
        </CardContent>
      </Card>

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
                defaultValue={15}
                className="w-20 rounded border bg-background px-3 py-1 text-right"
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
                defaultValue={500}
                className="w-24 rounded border bg-background px-3 py-1 text-right"
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
            <Switch defaultChecked />
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
            <Button variant="outline" size="sm">
              Connect
            </Button>
          </div>

          <div className="flex items-center justify-between">
            <div>
              <p className="font-medium">Discord Webhook</p>
              <p className="text-sm text-muted-foreground">
                Post updates to a Discord channel
              </p>
            </div>
            <Button variant="outline" size="sm">
              Configure
            </Button>
          </div>

          <div className="flex items-center justify-between">
            <div>
              <p className="font-medium">Email Notifications</p>
              <p className="text-sm text-muted-foreground">
                Daily summary and important alerts
              </p>
            </div>
            <Switch />
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
              <Button variant="outline" size="sm">
                Light
              </Button>
              <Button variant="default" size="sm">
                Dark
              </Button>
              <Button variant="outline" size="sm">
                System
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
