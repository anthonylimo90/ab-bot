import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { ConnectedWallet } from '@/types/api';
import { api } from '@/lib/api';

interface WalletStore {
  // State
  connectedWallets: ConnectedWallet[];
  primaryWallet: string | null;
  isLoading: boolean;
  error: string | null;

  // Actions
  fetchWallets: () => Promise<void>;
  connectWallet: (address: string, privateKey: string, label?: string) => Promise<ConnectedWallet>;
  disconnectWallet: (address: string) => Promise<void>;
  setPrimary: (address: string) => Promise<void>;
  clearError: () => void;
  reset: () => void;
}

const initialState = {
  connectedWallets: [],
  primaryWallet: null,
  isLoading: false,
  error: null,
};

export const useWalletStore = create<WalletStore>()(
  persist(
    (set, get) => ({
      ...initialState,

      fetchWallets: async () => {
        set({ isLoading: true, error: null });
        try {
          const wallets = await api.getConnectedWallets();
          const primary = wallets.find((w) => w.is_primary);
          set({
            connectedWallets: wallets,
            primaryWallet: primary?.address ?? null,
            isLoading: false,
          });
        } catch (error) {
          const message = error instanceof Error ? error.message : 'Failed to fetch wallets';
          set({ error: message, isLoading: false });
          throw error;
        }
      },

      connectWallet: async (address: string, privateKey: string, label?: string) => {
        set({ isLoading: true, error: null });
        try {
          const wallet = await api.connectWallet({ address, private_key: privateKey, label });
          set((state) => {
            const wallets = [...state.connectedWallets, wallet];
            return {
              connectedWallets: wallets,
              primaryWallet: wallet.is_primary ? wallet.address : state.primaryWallet,
              isLoading: false,
            };
          });
          return wallet;
        } catch (error) {
          const message = error instanceof Error ? error.message : 'Failed to connect wallet';
          set({ error: message, isLoading: false });
          throw error;
        }
      },

      disconnectWallet: async (address: string) => {
        set({ isLoading: true, error: null });
        try {
          await api.disconnectWallet(address);
          set((state) => {
            const wallets = state.connectedWallets.filter((w) => w.address !== address);
            const wasPrimary = state.primaryWallet === address;
            // If we removed the primary, pick the first remaining wallet
            const newPrimary = wasPrimary
              ? (wallets.find((w) => w.is_primary)?.address ?? wallets[0]?.address ?? null)
              : state.primaryWallet;
            return {
              connectedWallets: wallets,
              primaryWallet: newPrimary,
              isLoading: false,
            };
          });
        } catch (error) {
          const message = error instanceof Error ? error.message : 'Failed to disconnect wallet';
          set({ error: message, isLoading: false });
          throw error;
        }
      },

      setPrimary: async (address: string) => {
        set({ isLoading: true, error: null });
        try {
          await api.setPrimaryWallet(address);
          set((state) => ({
            connectedWallets: state.connectedWallets.map((w) => ({
              ...w,
              is_primary: w.address === address,
            })),
            primaryWallet: address,
            isLoading: false,
          }));
        } catch (error) {
          const message = error instanceof Error ? error.message : 'Failed to set primary wallet';
          set({ error: message, isLoading: false });
          throw error;
        }
      },

      clearError: () => set({ error: null }),

      reset: () => set(initialState),
    }),
    {
      name: 'ab-bot-wallets',
      partialize: (state) => ({
        // Only persist these fields - wallet data will be re-fetched from server
        primaryWallet: state.primaryWallet,
      }),
    }
  )
);

// Selectors for derived state
export const selectHasConnectedWallet = (state: WalletStore) => state.connectedWallets.length > 0;

export const selectPrimaryWallet = (state: WalletStore) => {
  const { connectedWallets, primaryWallet } = state;
  return connectedWallets.find((w) => w.address === primaryWallet);
};
