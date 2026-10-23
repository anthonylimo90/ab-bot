'use client';

import { cn } from '@/lib/utils';

interface LiveIndicatorProps {
  className?: string;
  label?: string;
  /** The latest refresh failed or is waiting for the network: what shows is
   * the last data read, not live. */
  stale?: boolean;
}

/** Whether any of the page's queries failed its latest refresh or is
 * waiting for the network (TanStack Query keeps the last data on screen). */
export function anyStale(
  ...queries: { isError: boolean; fetchStatus: string }[]
): boolean {
  return queries.some((query) => query.isError || query.fetchStatus === 'paused');
}

export function LiveIndicator({ className, label = 'LIVE', stale = false }: LiveIndicatorProps) {
  if (stale) {
    return (
      <div
        className={cn('flex items-center gap-1.5', className)}
        title="The latest refresh failed: this is the last data read"
      >
        <span className="relative inline-flex rounded-full h-2 w-2 bg-yellow-500" />
        <span className="text-xs font-medium text-yellow-600 dark:text-yellow-400 uppercase">
          Stale
        </span>
      </div>
    );
  }
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
