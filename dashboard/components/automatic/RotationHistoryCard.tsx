'use client';

import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import {
  ArrowUp,
  ArrowDown,
  RefreshCw,
  Plus,
  Minus,
  Check,
  Bell,
  ChevronDown,
  ChevronUp,
} from 'lucide-react';
import { useState } from 'react';
import { formatDistanceToNow } from 'date-fns';
import { shortenAddress, cn } from '@/lib/utils';
import type { RotationHistoryEntry } from '@/types/api';

interface RotationHistoryCardProps {
  history: RotationHistoryEntry[] | undefined;
  isLoading: boolean;
  onAcknowledge: (entryId: string) => void;
  isAcknowledging: boolean;
}

const actionConfig: Record<
  string,
  { icon: typeof ArrowUp; color: string; label: string }
> = {
  promote: { icon: ArrowUp, color: 'text-profit', label: 'Promoted' },
  demote: { icon: ArrowDown, color: 'text-loss', label: 'Demoted' },
  replace: { icon: RefreshCw, color: 'text-yellow-500', label: 'Replaced' },
  add: { icon: Plus, color: 'text-profit', label: 'Added' },
  remove: { icon: Minus, color: 'text-loss', label: 'Removed' },
};

export function RotationHistoryCard({
  history,
  isLoading,
  onAcknowledge,
  isAcknowledging,
}: RotationHistoryCardProps) {
  const [expandedId, setExpandedId] = useState<string | null>(null);

  if (isLoading) {
    return (
      <Card>
        <CardHeader>
          <Skeleton className="h-6 w-40" />
        </CardHeader>
        <CardContent className="space-y-3">
          {Array.from({ length: 5 }).map((_, i) => (
            <Skeleton key={i} className="h-16 w-full" />
          ))}
        </CardContent>
      </Card>
    );
  }

  const unacknowledgedCount = history?.filter((h) => !h.acknowledged).length ?? 0;

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between">
        <CardTitle className="flex items-center gap-2">
          <RefreshCw className="h-5 w-5" />
          Rotation History
          {unacknowledgedCount > 0 && (
            <Badge variant="destructive" className="ml-2">
              <Bell className="h-3 w-3 mr-1" />
              {unacknowledgedCount} new
            </Badge>
          )}
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        {!history || history.length === 0 ? (
          <p className="text-sm text-muted-foreground text-center py-4">
            No rotation history yet. The optimizer will make changes based on your criteria.
          </p>
        ) : (
          history.slice(0, 10).map((entry) => {
            const config = actionConfig[entry.action] ?? actionConfig.replace;
            const Icon = config.icon;
            const isExpanded = expandedId === entry.id;

            return (
              <div
                key={entry.id}
                className={cn(
                  'rounded-lg border p-3 transition-colors',
                  !entry.acknowledged && 'border-primary bg-primary/5'
                )}
              >
                <div className="flex items-start justify-between">
                  <div className="flex items-start gap-3">
                    <div
                      className={cn(
                        'flex h-8 w-8 items-center justify-center rounded-full',
                        config.color,
                        'bg-muted'
                      )}
                    >
                      <Icon className="h-4 w-4" />
                    </div>
                    <div>
                      <div className="flex items-center gap-2">
                        <Badge variant="outline" className="text-xs">
                          {config.label}
                        </Badge>
                        {entry.is_automatic && (
                          <Badge variant="secondary" className="text-xs">
                            Auto
                          </Badge>
                        )}
                      </div>
                      <p className="text-sm mt-1">
                        {entry.wallet_in && (
                          <span className="font-mono text-profit">
                            {shortenAddress(entry.wallet_in)}
                          </span>
                        )}
                        {entry.wallet_in && entry.wallet_out && ' replaced '}
                        {entry.wallet_out && !entry.wallet_in && 'Wallet '}
                        {entry.wallet_out && (
                          <span className="font-mono text-loss">
                            {shortenAddress(entry.wallet_out)}
                          </span>
                        )}
                      </p>
                      <p className="text-xs text-muted-foreground mt-1">
                        {formatDistanceToNow(new Date(entry.created_at), {
                          addSuffix: true,
                        })}
                      </p>
                    </div>
                  </div>
                  <div className="flex items-center gap-2">
                    {!entry.acknowledged && (
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => onAcknowledge(entry.id)}
                        disabled={isAcknowledging}
                      >
                        <Check className="h-3 w-3 mr-1" />
                        Ack
                      </Button>
                    )}
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() =>
                        setExpandedId(isExpanded ? null : entry.id)
                      }
                    >
                      {isExpanded ? (
                        <ChevronUp className="h-4 w-4" />
                      ) : (
                        <ChevronDown className="h-4 w-4" />
                      )}
                    </Button>
                  </div>
                </div>

                {/* Expanded Details */}
                {isExpanded && (
                  <div className="mt-3 pt-3 border-t text-sm">
                    <p className="text-muted-foreground mb-2">
                      <strong>Reason:</strong> {entry.reason}
                    </p>
                    {entry.evidence && Object.keys(entry.evidence).length > 0 && (
                      <pre className="text-xs bg-muted p-2 rounded overflow-x-auto">
                        {JSON.stringify(entry.evidence, null, 2)}
                      </pre>
                    )}
                  </div>
                )}
              </div>
            );
          })
        )}
      </CardContent>
    </Card>
  );
}
