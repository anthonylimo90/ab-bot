'use client';

import { cn } from '@/lib/utils';

interface LiveIndicatorProps {
  className?: string;
  label?: string;
}

export function LiveIndicator({ className, label = 'LIVE' }: LiveIndicatorProps) {
  return (
    <div className={cn('flex items-center gap-1.5', className)}>
      <span className="relative flex h-2 w-2">
        <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-profit opacity-75" />
        <span className="relative inline-flex rounded-full h-2 w-2 bg-profit" />
      </span>
      <span className="text-xs font-medium text-profit uppercase">{label}</span>
    </div>
  );
}
