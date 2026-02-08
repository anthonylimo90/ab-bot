import { create } from 'zustand';
import api from '@/lib/api';
import type { DemoPosition as ApiDemoPosition, DemoBalance } from '@/types/api';

// Local interface that matches API response but with computed fields
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
  isLoading: boolean;
  error: string | null;
  lastFetchedWorkspaceId: string | null;

  // Data fetching
  fetchPositions: () => Promise<void>;
  fetchBalance: () => Promise<void>;
  fetchAll: () => Promise<void>;

  // Position management
  addPosition: (position: Omit<DemoPosition, 'id'>) => Promise<void>;
  closePosition: (id: string, exitPrice: number) => Promise<void>;
  updatePrice: (id: string, price: number) => void;
  updatePrices: (updates: { marketId: string; outcome: 'yes' | 'no'; price: number }[]) => void;
  deletePosition: (id: string) => Promise<void>;

  // Balance management
  updateBalance: (amount: number) => Promise<void>;
  reset: () => Promise<void>;

  // Computed values
  getTotalValue: () => number;
  getTotalPnl: () => number;
  getTotalPnlPercent: () => number;

  // Internal
  setWorkspaceId: (workspaceId: string | null) => void;
}

const DEFAULT_BALANCE = 10000;

// Convert API response to local format
function apiToLocal(apiPosition: ApiDemoPosition): DemoPosition {
  return {
    id: apiPosition.id,
    walletAddress: apiPosition.wallet_address,
    walletLabel: apiPosition.wallet_label,
    marketId: apiPosition.market_id,
    marketQuestion: apiPosition.market_question,
    outcome: apiPosition.outcome,
    quantity: apiPosition.quantity,
    entryPrice: apiPosition.entry_price,
    currentPrice: apiPosition.current_price ?? apiPosition.entry_price,
    openedAt: apiPosition.opened_at,
    closedAt: apiPosition.closed_at,
    exitPrice: apiPosition.exit_price,
    realizedPnl: apiPosition.realized_pnl,
  };
}

export const useDemoPortfolioStore = create<DemoPortfolioStore>()((set, get) => ({
  positions: [],
  closedPositions: [],
  balance: DEFAULT_BALANCE,
  initialBalance: DEFAULT_BALANCE,
  isLoading: false,
  error: null,
  lastFetchedWorkspaceId: null,

  setWorkspaceId: (workspaceId: string | null) => {
    const state = get();
    if (state.lastFetchedWorkspaceId !== workspaceId) {
      // Clear positions when workspace changes
      set({
        positions: [],
        closedPositions: [],
        balance: DEFAULT_BALANCE,
        initialBalance: DEFAULT_BALANCE,
        lastFetchedWorkspaceId: workspaceId,
      });
    }
  },

  fetchPositions: async () => {
    set({ isLoading: true, error: null });
    try {
      const [openPositions, closedPositions] = await Promise.all([
        api.listDemoPositions({ status: 'open' }),
        api.listDemoPositions({ status: 'closed' }),
      ]);

      set({
        positions: openPositions.map(apiToLocal),
        closedPositions: closedPositions.map(apiToLocal),
        isLoading: false,
      });
    } catch (err) {
      set({
        error: err instanceof Error ? err.message : 'Failed to fetch demo positions',
        isLoading: false,
      });
    }
  },

  fetchBalance: async () => {
    try {
      const balanceData = await api.getDemoBalance();
      set({
        balance: balanceData.balance,
        initialBalance: balanceData.initial_balance,
      });
    } catch (err) {
      // If balance doesn't exist yet, use default
      console.warn('Failed to fetch demo balance, using default');
    }
  },

  fetchAll: async () => {
    const state = get();
    set({ isLoading: true, error: null });
    try {
      await Promise.all([state.fetchPositions(), state.fetchBalance()]);
    } catch (err) {
      set({
        error: err instanceof Error ? err.message : 'Failed to fetch demo data',
        isLoading: false,
      });
    }
  },

  addPosition: async (position) => {
    set({ isLoading: true, error: null });
    try {
      // Create position via API
      const created = await api.createDemoPosition({
        wallet_address: position.walletAddress,
        wallet_label: position.walletLabel,
        market_id: position.marketId,
        market_question: position.marketQuestion,
        outcome: position.outcome,
        quantity: position.quantity,
        entry_price: position.entryPrice,
        current_price: position.currentPrice,
        opened_at: position.openedAt,
      });

      const latestBalance = await api.getDemoBalance();

      set((state) => ({
        positions: [...state.positions, apiToLocal(created)],
        balance: latestBalance.balance,
        initialBalance: latestBalance.initial_balance,
        isLoading: false,
      }));
    } catch (err) {
      set({
        error: err instanceof Error ? err.message : 'Failed to add position',
        isLoading: false,
      });
      throw err;
    }
  },

  closePosition: async (id, exitPrice) => {
    const state = get();
    const position = state.positions.find((p) => p.id === id);

    if (!position) return;

    set({ isLoading: true, error: null });
    try {
      const entryValue = position.quantity * position.entryPrice;
      const realizedPnl = position.quantity * exitPrice - entryValue;
      const closedAt = new Date().toISOString();

      // Update position via API
      const updated = await api.updateDemoPosition(id, {
        closed_at: closedAt,
        exit_price: exitPrice,
        realized_pnl: realizedPnl,
        current_price: exitPrice,
      });

      const latestBalance = await api.getDemoBalance();

      const closedPosition = apiToLocal(updated);

      set((s) => ({
        positions: s.positions.filter((p) => p.id !== id),
        closedPositions: [...s.closedPositions, closedPosition],
        balance: latestBalance.balance,
        initialBalance: latestBalance.initial_balance,
        isLoading: false,
      }));
    } catch (err) {
      set({
        error: err instanceof Error ? err.message : 'Failed to close position',
        isLoading: false,
      });
      throw err;
    }
  },

  // Local-only price updates (optimistic, no API call for performance)
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

  deletePosition: async (id) => {
    set({ isLoading: true, error: null });
    try {
      await api.deleteDemoPosition(id);
      set((state) => ({
        positions: state.positions.filter((p) => p.id !== id),
        closedPositions: state.closedPositions.filter((p) => p.id !== id),
        isLoading: false,
      }));
    } catch (err) {
      set({
        error: err instanceof Error ? err.message : 'Failed to delete position',
        isLoading: false,
      });
      throw err;
    }
  },

  updateBalance: async (amount) => {
    set({ isLoading: true, error: null });
    try {
      const newBalance = get().balance + amount;
      await api.updateDemoBalance(newBalance);
      set({ balance: newBalance, isLoading: false });
    } catch (err) {
      set({
        error: err instanceof Error ? err.message : 'Failed to update balance',
        isLoading: false,
      });
      throw err;
    }
  },

  reset: async () => {
    set({ isLoading: true, error: null });
    try {
      const balanceData = await api.resetDemoPortfolio();
      set({
        positions: [],
        closedPositions: [],
        balance: balanceData.balance,
        initialBalance: balanceData.initial_balance,
        isLoading: false,
      });
    } catch (err) {
      set({
        error: err instanceof Error ? err.message : 'Failed to reset portfolio',
        isLoading: false,
      });
      throw err;
    }
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
}));
