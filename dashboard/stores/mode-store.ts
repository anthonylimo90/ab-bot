import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export type TradingMode = 'demo' | 'live';

interface ModeStore {
  mode: TradingMode;
  demoBalance: number;
  initialDemoBalance: number;
  setMode: (mode: TradingMode) => void;
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

      setMode: (mode) => set({ mode }),

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
