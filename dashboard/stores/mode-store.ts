import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { QueryClient } from '@tanstack/react-query';

export type TradingMode = 'demo' | 'live';

interface ModeStore {
  mode: TradingMode;
  setMode: (mode: TradingMode, queryClient?: QueryClient) => Promise<void>;
  isDemo: () => boolean;
  isLive: () => boolean;
}

export const useModeStore = create<ModeStore>()(
  persist(
    (set, get) => ({
      mode: 'demo',

      setMode: async (mode, queryClient) => {
        const oldMode = get().mode;
        if (oldMode === mode) return;

        // 1. Invalidate queries first to ensure clean cache state
        if (queryClient) {
          await Promise.all([
            queryClient.invalidateQueries({ queryKey: ['positions'] }),
            queryClient.invalidateQueries({ queryKey: ['wallets'] }),
            queryClient.invalidateQueries({ queryKey: ['allocations'] }),
            queryClient.invalidateQueries({ queryKey: ['portfolio'] }),
            queryClient.invalidateQueries({ queryKey: ['discover'] }),
            queryClient.invalidateQueries({ queryKey: ['rotation-history'] }),
          ]);
        }

        // 2. Update mode after cache is invalidated
        set({ mode });

        // 3. Then clear demo store if switching to live
        if (mode === 'live') {
          // Import demo store dynamically to avoid circular dependency
          const { useDemoPortfolioStore } = await import('./demo-portfolio-store');
          const demoStore = useDemoPortfolioStore.getState();
          demoStore.clearPositions();
        }
      },

      isDemo: () => get().mode === 'demo',
      isLive: () => get().mode === 'live',
    }),
    {
      name: 'ab-bot-mode',
      partialize: (state) => ({
        mode: state.mode,
      }),
    }
  )
);
