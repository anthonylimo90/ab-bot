'use client';

import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Skeleton } from '@/components/ui/skeleton';
import { Settings, Play, Clock, CheckCircle, XCircle, Loader2 } from 'lucide-react';
import { formatDistanceToNow, format } from 'date-fns';
import type { OptimizerStatus } from '@/types/api';

interface OptimizerStatusCardProps {
  status: OptimizerStatus | undefined;
  isLoading: boolean;
  onTriggerOptimization: () => void;
  isTriggering: boolean;
  onOpenSettings: () => void;
  canTrigger: boolean; // owner/admin only
}

export function OptimizerStatusCard({
  status,
  isLoading,
  onTriggerOptimization,
  isTriggering,
  onOpenSettings,
  canTrigger,
}: OptimizerStatusCardProps) {
  if (isLoading) {
    return (
      <Card>
        <CardHeader>
          <Skeleton className="h-6 w-40" />
        </CardHeader>
        <CardContent className="space-y-4">
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-20 w-full" />
        </CardContent>
      </Card>
    );
  }

  const lastRunText = status?.last_run_at
    ? formatDistanceToNow(new Date(status.last_run_at), { addSuffix: true })
    : 'Never';

  const nextRunText = status?.next_run_at
    ? formatDistanceToNow(new Date(status.next_run_at), { addSuffix: true })
    : 'N/A';

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between">
        <CardTitle className="flex items-center gap-2">
          Optimizer Status
          <Badge variant={status?.enabled ? 'default' : 'secondary'}>
            {status?.enabled ? (
              <>
                <CheckCircle className="h-3 w-3 mr-1" />
                Enabled
              </>
            ) : (
              <>
                <XCircle className="h-3 w-3 mr-1" />
                Disabled
              </>
            )}
          </Badge>
        </CardTitle>
        <Button variant="ghost" size="icon" onClick={onOpenSettings}>
          <Settings className="h-4 w-4" />
        </Button>
      </CardHeader>
      <CardContent className="space-y-4">
        {/* Timing Info */}
        <div className="grid grid-cols-2 gap-4 text-sm">
          <div>
            <p className="text-muted-foreground flex items-center gap-1">
              <Clock className="h-3 w-3" />
              Last Run
            </p>
            <p
              className="font-medium"
              title={
                status?.last_run_at
                  ? format(new Date(status.last_run_at), 'PPpp')
                  : undefined
              }
            >
              {lastRunText}
            </p>
          </div>
          <div>
            <p className="text-muted-foreground flex items-center gap-1">
              <Clock className="h-3 w-3" />
              Next Run
            </p>
            <p
              className="font-medium"
              title={
                status?.next_run_at
                  ? format(new Date(status.next_run_at), 'PPpp')
                  : undefined
              }
            >
              {nextRunText}
            </p>
          </div>
        </div>

        {/* Criteria */}
        <div className="border rounded-lg p-3">
          <p className="text-sm font-medium mb-2">Selection Criteria</p>
          <div className="grid grid-cols-2 gap-2 text-sm">
            <div className="flex justify-between">
              <span className="text-muted-foreground">Min ROI:</span>
              <span>{status?.criteria.min_roi_30d?.toFixed(1) ?? '0'}%</span>
            </div>
            <div className="flex justify-between">
              <span className="text-muted-foreground">Min Sharpe:</span>
              <span>{status?.criteria.min_sharpe?.toFixed(2) ?? '0'}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-muted-foreground">Min Win Rate:</span>
              <span>{status?.criteria.min_win_rate?.toFixed(0) ?? '0'}%</span>
            </div>
            <div className="flex justify-between">
              <span className="text-muted-foreground">Min Trades:</span>
              <span>{status?.criteria.min_trades_30d ?? 0}</span>
            </div>
          </div>
        </div>

        {/* Wallet Counts */}
        <div className="flex items-center justify-between text-sm">
          <div className="flex items-center gap-4">
            <span>
              <span className="font-medium text-profit">{status?.active_wallet_count ?? 0}</span>
              <span className="text-muted-foreground ml-1">Active</span>
            </span>
            <span>
              <span className="font-medium">{status?.bench_wallet_count ?? 0}</span>
              <span className="text-muted-foreground ml-1">Bench</span>
            </span>
          </div>
          <span className="text-muted-foreground">
            Interval: {status?.interval_hours ?? 24}h
          </span>
        </div>

        {/* Run Now Button */}
        {canTrigger && (
          <Button
            onClick={onTriggerOptimization}
            disabled={isTriggering || !status?.enabled}
            className="w-full"
          >
            {isTriggering ? (
              <>
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                Running...
              </>
            ) : (
              <>
                <Play className="mr-2 h-4 w-4" />
                Run Now
              </>
            )}
          </Button>
        )}
      </CardContent>
    </Card>
  );
}
