'use client';

import { cn } from '@/lib/utils';
import { Wifi, WifiOff, Loader2 } from 'lucide-react';
import type { ConnectionStatus as Status } from '@/hooks/useWebSocket';

interface ConnectionStatusProps {
  status: Status;
  className?: string;
  showLabel?: boolean;
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

export function ConnectionStatus({ status, className, showLabel = false }: ConnectionStatusProps) {
  const config = statusConfig[status];
  const Icon = config.icon;

  return (
    <div
      className={cn(
        'flex items-center gap-2 rounded-full px-2 py-1',
        config.bgColor,
        className
      )}
      role="status"
      aria-label={config.label}
    >
      <Icon
        className={cn(
          'h-3 w-3',
          config.color,
          status === 'connecting' && 'animate-spin'
        )}
      />
      {showLabel && (
        <span className={cn('text-xs font-medium', config.color)}>
          {config.label}
        </span>
      )}
    </div>
  );
}
