import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { Wallet, CopySettings, DecisionBrief, WalletTier, CopyBehavior } from '@/types/api';

export interface RosterWallet {
  address: string;
  label?: string;
  tier: WalletTier;
  copySettings: CopySettings;
  decisionBrief?: DecisionBrief;
  // Performance metrics
  roi30d: number;
  sharpe: number;
  winRate: number;
  trades: number;
  maxDrawdown: number;
  confidence: number;
  // Status
  addedAt: string;
  lastActivity?: string;
  // Automation fields
  pinned?: boolean;
  pinnedAt?: string;
  probationUntil?: string;
  isAutoSelected?: boolean;
  consecutiveLosses?: number;
}

interface RosterState {
  // State
  activeWallets: RosterWallet[];
  benchWallets: RosterWallet[];

  // Constants
  maxActiveWallets: number;
  maxBenchWallets: number;

  // Computed
  isRosterFull: () => boolean;
  canPromote: (address: string) => boolean;
  getTotalAllocation: () => number;
  getWallet: (address: string) => RosterWallet | undefined;

  // Actions
  addToActive: (wallet: RosterWallet) => boolean;
  addToBench: (wallet: RosterWallet) => void;
  promoteToActive: (address: string) => boolean;
  demoteToBench: (address: string) => void;
  removeFromBench: (address: string) => void;
  removeFromActive: (address: string) => void;
  updateCopySettings: (address: string, settings: Partial<CopySettings>) => void;
  updateDecisionBrief: (address: string, brief: DecisionBrief) => void;
}

const defaultCopySettings: CopySettings = {
  copy_behavior: 'events_only',
  allocation_pct: 10,
  max_position_size: 100,
};

export const useRosterStore = create<RosterState>()(
  persist(
    (set, get) => ({
      activeWallets: [],
      benchWallets: [],
      maxActiveWallets: 5,
      maxBenchWallets: 20,

      isRosterFull: () => get().activeWallets.length >= get().maxActiveWallets,

      canPromote: (address: string) => {
        const state = get();
        const isOnBench = state.benchWallets.some((w) => w.address === address);
        return isOnBench && !state.isRosterFull();
      },

      getTotalAllocation: () => {
        return get().activeWallets.reduce(
          (sum, w) => sum + w.copySettings.allocation_pct,
          0
        );
      },

      getWallet: (address: string) => {
        const state = get();
        return (
          state.activeWallets.find((w) => w.address === address) ||
          state.benchWallets.find((w) => w.address === address)
        );
      },

      addToActive: (wallet: RosterWallet) => {
        const state = get();
        if (state.isRosterFull()) return false;
        if (state.activeWallets.some((w) => w.address === wallet.address)) return false;

        // Remove from bench if present
        const updatedBench = state.benchWallets.filter(
          (w) => w.address !== wallet.address
        );

        set({
          activeWallets: [
            ...state.activeWallets,
            { ...wallet, tier: 'active', addedAt: new Date().toISOString() },
          ],
          benchWallets: updatedBench,
        });
        return true;
      },

      addToBench: (wallet: RosterWallet) => {
        const state = get();
        if (state.benchWallets.length >= state.maxBenchWallets) return;
        if (state.benchWallets.some((w) => w.address === wallet.address)) return;
        if (state.activeWallets.some((w) => w.address === wallet.address)) return;

        set({
          benchWallets: [
            ...state.benchWallets,
            { ...wallet, tier: 'bench', addedAt: new Date().toISOString() },
          ],
        });
      },

      promoteToActive: (address: string) => {
        const state = get();
        if (state.isRosterFull()) return false;

        const wallet = state.benchWallets.find((w) => w.address === address);
        if (!wallet) return false;

        set({
          activeWallets: [
            ...state.activeWallets,
            { ...wallet, tier: 'active' },
          ],
          benchWallets: state.benchWallets.filter((w) => w.address !== address),
        });
        return true;
      },

      demoteToBench: (address: string) => {
        const state = get();
        const wallet = state.activeWallets.find((w) => w.address === address);
        if (!wallet) return;

        set({
          benchWallets: [...state.benchWallets, { ...wallet, tier: 'bench' }],
          activeWallets: state.activeWallets.filter((w) => w.address !== address),
        });
      },

      removeFromBench: (address: string) => {
        set({
          benchWallets: get().benchWallets.filter((w) => w.address !== address),
        });
      },

      removeFromActive: (address: string) => {
        set({
          activeWallets: get().activeWallets.filter((w) => w.address !== address),
        });
      },

      updateCopySettings: (address: string, settings: Partial<CopySettings>) => {
        const state = get();

        const updateWallet = (wallets: RosterWallet[]) =>
          wallets.map((w) =>
            w.address === address
              ? { ...w, copySettings: { ...w.copySettings, ...settings } }
              : w
          );

        set({
          activeWallets: updateWallet(state.activeWallets),
          benchWallets: updateWallet(state.benchWallets),
        });
      },

      updateDecisionBrief: (address: string, brief: DecisionBrief) => {
        const state = get();

        const updateWallet = (wallets: RosterWallet[]) =>
          wallets.map((w) =>
            w.address === address ? { ...w, decisionBrief: brief } : w
          );

        set({
          activeWallets: updateWallet(state.activeWallets),
          benchWallets: updateWallet(state.benchWallets),
        });
      },
    }),
    {
      name: 'ab-bot-roster',
      partialize: (state) => ({
        activeWallets: state.activeWallets,
        benchWallets: state.benchWallets,
      }),
    }
  )
);

// Helper to create a RosterWallet from wallet discovery data
export function createRosterWallet(
  address: string,
  metrics: {
    roi30d: number;
    sharpe: number;
    winRate: number;
    trades: number;
    maxDrawdown: number;
    confidence: number;
  },
  tier: WalletTier = 'bench',
  copySettings: Partial<CopySettings> = {}
): RosterWallet {
  return {
    address,
    tier,
    copySettings: { ...defaultCopySettings, ...copySettings },
    roi30d: metrics.roi30d,
    sharpe: metrics.sharpe,
    winRate: metrics.winRate,
    trades: metrics.trades,
    maxDrawdown: metrics.maxDrawdown,
    confidence: metrics.confidence,
    addedAt: new Date().toISOString(),
  };
}
