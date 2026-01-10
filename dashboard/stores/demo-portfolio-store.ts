import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export interface DemoPosition {
  id: string;
  walletAddress: string;
  walletLabel?: string;
  marketId: string;
  marketQuestion?: string;
  outcome: 'yes' | 'no';
  quantity: number;
  entryPrice: number;
  currentPrice: number;
  openedAt: string;
  closedAt?: string;
  exitPrice?: number;
  realizedPnl?: number;
}

interface DemoPortfolioStore {
  positions: DemoPosition[];
  closedPositions: DemoPosition[];
  balance: number;
  initialBalance: number;

  // Position management
  addPosition: (position: Omit<DemoPosition, 'id'>) => void;
  closePosition: (id: string, exitPrice: number) => void;
  updatePrice: (id: string, price: number) => void;
  updatePrices: (updates: { marketId: string; outcome: 'yes' | 'no'; price: number }[]) => void;

  // Balance management
  updateBalance: (amount: number) => void;
  reset: () => void;

  // Computed values
  getTotalValue: () => number;
  getTotalPnl: () => number;
  getTotalPnlPercent: () => number;
}

const DEFAULT_BALANCE = 10000;

export const useDemoPortfolioStore = create<DemoPortfolioStore>()(
  persist(
    (set, get) => ({
      positions: [],
      closedPositions: [],
      balance: DEFAULT_BALANCE,
      initialBalance: DEFAULT_BALANCE,

      addPosition: (position) => {
        const id = `demo-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
        const cost = position.quantity * position.entryPrice;

        set((state) => ({
          positions: [...state.positions, { ...position, id }],
          balance: state.balance - cost,
        }));
      },

      closePosition: (id, exitPrice) => {
        const state = get();
        const position = state.positions.find((p) => p.id === id);

        if (!position) return;

        const exitValue = position.quantity * exitPrice;
        const entryValue = position.quantity * position.entryPrice;
        const realizedPnl = exitValue - entryValue;

        const closedPosition: DemoPosition = {
          ...position,
          closedAt: new Date().toISOString(),
          exitPrice,
          realizedPnl,
          currentPrice: exitPrice,
        };

        set((state) => ({
          positions: state.positions.filter((p) => p.id !== id),
          closedPositions: [...state.closedPositions, closedPosition],
          balance: state.balance + exitValue,
        }));
      },

      updatePrice: (id, price) => {
        set((state) => ({
          positions: state.positions.map((p) =>
            p.id === id ? { ...p, currentPrice: price } : p
          ),
        }));
      },

      updatePrices: (updates) => {
        set((state) => ({
          positions: state.positions.map((p) => {
            const update = updates.find(
              (u) => u.marketId === p.marketId && u.outcome === p.outcome
            );
            return update ? { ...p, currentPrice: update.price } : p;
          }),
        }));
      },

      updateBalance: (amount) => {
        set((state) => ({
          balance: state.balance + amount,
        }));
      },

      reset: () => {
        set({
          positions: [],
          closedPositions: [],
          balance: DEFAULT_BALANCE,
          initialBalance: DEFAULT_BALANCE,
        });
      },

      getTotalValue: () => {
        const state = get();
        const positionValue = state.positions.reduce(
          (sum, p) => sum + p.quantity * p.currentPrice,
          0
        );
        return state.balance + positionValue;
      },

      getTotalPnl: () => {
        const state = get();
        return state.getTotalValue() - state.initialBalance;
      },

      getTotalPnlPercent: () => {
        const state = get();
        const pnl = state.getTotalPnl();
        return (pnl / state.initialBalance) * 100;
      },
    }),
    {
      name: 'ab-bot-demo-portfolio',
      partialize: (state) => ({
        positions: state.positions,
        closedPositions: state.closedPositions,
        balance: state.balance,
        initialBalance: state.initialBalance,
      }),
    }
  )
);
