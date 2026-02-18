'use client';

import { cn } from '@/lib/utils';
import { Wifi, WifiOff, Loader2, RefreshCw } from 'lucide-react';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import type { ConnectionStatus as Status } from '@/hooks/useWebSocket';

interface ConnectionStatusProps {
  status: Status;
  className?: string;
  showLabel?: boolean;
  /** Current reconnection attempt (only meaningful when status is 'connecting') */
  reconnectAttempt?: number;
  /** Maximum reconnection attempts */
  maxReconnectAttempts?: number;
}

const statusConfig = {
  connected: {
    icon: Wifi,
    color: 'text-profit',
    bgColor: 'bg-profit/10',
    label: 'Connected',
  },
  connecting: {
    icon: Loader2,
    color: 'text-yellow-500',
    bgColor: 'bg-yellow-500/10',
    label: 'Connecting...',
  },
  disconnected: {
    icon: WifiOff,
    color: 'text-muted-foreground',
    bgColor: 'bg-muted',
    label: 'Disconnected',
  },
  error: {
    icon: WifiOff,
    color: 'text-loss',
    bgColor: 'bg-loss/10',
    label: 'Connection Error',
  },
};

export function ConnectionStatus({
  status,
  className,
  showLabel = false,
  reconnectAttempt,
  maxReconnectAttempts,
}: ConnectionStatusProps) {
  const config = statusConfig[status];
  const isReconnecting =
    status === 'connecting' && reconnectAttempt != null && reconnectAttempt > 0;
  const Icon = isReconnecting ? RefreshCw : config.icon;
  const label = isReconnecting
    ? `Reconnecting (${reconnectAttempt}/${maxReconnectAttempts ?? '?'})...`
    : config.label;

  const indicator = (
    <div
      className={cn(
        'flex items-center gap-2 rounded-full px-2 py-1',
        config.bgColor,
        className
      )}
      role="status"
      aria-label={label}
    >
      <Icon
        className={cn(
          'h-3 w-3',
          config.color,
          (status === 'connecting' || isReconnecting) && 'animate-spin'
        )}
      />
      {showLabel && (
        <span className={cn('text-xs font-medium', config.color)}>
          {label}
        </span>
      )}
    </div>
  );

  // Show tooltip with reconnection details when reconnecting
  if (isReconnecting) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>{indicator}</TooltipTrigger>
        <TooltipContent>
          <p className="text-xs">
            Reconnecting: attempt {reconnectAttempt} of{' '}
            {maxReconnectAttempts ?? '?'}
          </p>
        </TooltipContent>
      </Tooltip>
    );
  }

  return indicator;
}
