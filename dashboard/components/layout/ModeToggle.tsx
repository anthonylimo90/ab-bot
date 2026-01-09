'use client';

import { useModeStore } from '@/stores/mode-store';
import { cn } from '@/lib/utils';

export function ModeToggle() {
  const { mode, setMode, demoBalance, initialDemoBalance } = useModeStore();
  const isDemo = mode === 'demo';

  const pnl = demoBalance - initialDemoBalance;
  const pnlPercent = ((pnl / initialDemoBalance) * 100).toFixed(1);

  return (
    <div className="flex items-center gap-3">
      <button
        onClick={() => setMode(isDemo ? 'live' : 'demo')}
        className={cn(
          'relative flex items-center gap-2 rounded-full px-4 py-2 text-sm font-medium transition-all',
          isDemo
            ? 'bg-demo/10 text-demo hover:bg-demo/20'
            : 'bg-live/10 text-live hover:bg-live/20'
        )}
      >
        <span
          className={cn(
            'h-2 w-2 rounded-full animate-pulse',
            isDemo ? 'bg-demo' : 'bg-live'
          )}
        />
        <span>{isDemo ? 'Demo Mode' : 'Live Mode'}</span>
      </button>

      {isDemo && (
        <div className="hidden sm:flex items-center gap-2 text-sm">
          <span className="text-muted-foreground">Balance:</span>
          <span className="font-medium tabular-nums">
            ${demoBalance.toLocaleString()}
          </span>
          {pnl !== 0 && (
            <span
              className={cn(
                'tabular-nums',
                pnl > 0 ? 'text-profit' : 'text-loss'
              )}
            >
              ({pnl > 0 ? '+' : ''}
              {pnlPercent}%)
            </span>
          )}
        </div>
      )}
    </div>
  );
}
