import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { QueryClient } from '@tanstack/react-query';

export type TradingMode = 'demo' | 'live';

interface ModeStore {
  mode: TradingMode;
  demoBalance: number;
  initialDemoBalance: number;
  setMode: (mode: TradingMode, queryClient?: QueryClient) => Promise<void>;
  updateDemoBalance: (balance: number) => void;
  resetDemoBalance: () => void;
  isDemo: () => boolean;
  isLive: () => boolean;
}

const DEFAULT_DEMO_BALANCE = 10000;

export const useModeStore = create<ModeStore>()(
  persist(
    (set, get) => ({
      mode: 'demo',
      demoBalance: DEFAULT_DEMO_BALANCE,
      initialDemoBalance: DEFAULT_DEMO_BALANCE,

      setMode: async (mode, queryClient) => {
        const oldMode = get().mode;
        if (oldMode === mode) return;

        // 1. Update mode
        set({ mode });

        // 2. Invalidate queries first to ensure clean cache state
        if (queryClient) {
          // Invalidate only mode-specific queries for efficiency
          await Promise.all([
            queryClient.invalidateQueries({ queryKey: ['positions'] }),
            queryClient.invalidateQueries({ queryKey: ['wallets'] }),
            queryClient.invalidateQueries({ queryKey: ['allocations'] }),
            queryClient.invalidateQueries({ queryKey: ['portfolio'] }),
            queryClient.invalidateQueries({ queryKey: ['discover'] }),
            queryClient.invalidateQueries({ queryKey: ['rotation-history'] }),
          ]);
        }

        // 3. Then clear demo store if switching to live
        if (mode === 'live') {
          // Import demo store dynamically to avoid circular dependency
          const { useDemoPortfolioStore } = await import('./demo-portfolio-store');
          const demoStore = useDemoPortfolioStore.getState();
          demoStore.clearPositions();
        }
      },

      updateDemoBalance: (balance) => set({ demoBalance: balance }),

      resetDemoBalance: () =>
        set({
          demoBalance: DEFAULT_DEMO_BALANCE,
          initialDemoBalance: DEFAULT_DEMO_BALANCE,
        }),

      isDemo: () => get().mode === 'demo',
      isLive: () => get().mode === 'live',
    }),
    {
      name: 'ab-bot-mode',
      partialize: (state) => ({
        mode: state.mode,
        demoBalance: state.demoBalance,
        initialDemoBalance: state.initialDemoBalance,
      }),
    }
  )
);
