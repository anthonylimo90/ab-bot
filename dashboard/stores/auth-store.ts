import { create } from 'zustand';
import { persist, createJSONStorage } from 'zustand/middleware';
import type { User } from '@/types/api';
import api, { ApiHttpError } from '@/lib/api';
import { useToastStore } from '@/stores/toast-store';
import { queryClient } from '@/lib/queryClient';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { useWalletStore } from '@/stores/wallet-store';

interface AuthStore {
  token: string | null;
  user: User | null;
  isLoading: boolean;
  isAuthenticated: boolean;
  _hasHydrated: boolean;

  setAuth: (token: string, user: User) => void;
  logout: () => Promise<boolean>;
  checkAuth: () => Promise<void>;
  sessionExpired: (rejectedToken: string) => void;
  setHasHydrated: (state: boolean) => void;
  isPlatformAdmin: () => boolean;
}

function resetSessionContext() {
  queryClient.clear();
  useWorkspaceStore.getState().reset();
  useWalletStore.getState().reset();
}

export const useAuthStore = create<AuthStore>()(
  persist(
    (set, get) => ({
      token: null,
      user: null,
      isLoading: false,
      isAuthenticated: false,
      _hasHydrated: false,

      setHasHydrated: (state) => {
        set({ _hasHydrated: state });
      },

      setAuth: (token, user) => {
        resetSessionContext();
        api.setToken(token);
        set({ token, user, isAuthenticated: true, isLoading: false });
      },

      logout: async () => {
        const token = get().token;
        try {
          if (token) await api.logout();
        } catch (error) {
          if (!(error instanceof ApiHttpError && error.isUnauthorized)) {
            useToastStore.getState().error('Logout failed', 'Your session is still active. Please try again.');
            return false;
          }
        }
        // A concurrent sign-in must not be cleared by an older logout request.
        if (get().token !== token) return false;
        api.clearToken();
        resetSessionContext();
        set({ token: null, user: null, isAuthenticated: false, isLoading: false });
        return true;
      },

      checkAuth: async () => {
        const { token } = get();
        if (!token) {
          resetSessionContext();
          set({ token: null, user: null, isLoading: false, isAuthenticated: false });
          return;
        }

        set({ isLoading: true });

        // Set token on API client
        api.setToken(token);

        try {
          const user = await api.getCurrentUser();
          set({ user, isAuthenticated: true, isLoading: false });
        } catch (error) {
          if (error instanceof ApiHttpError && error.isUnauthorized) {
            // The server rejected the token: it is invalid, expired or revoked.
            // A concurrent sign-in must not be cleared by this older check.
            if (get().token === token) {
              api.clearToken();
              resetSessionContext();
              set({ token: null, user: null, isAuthenticated: false, isLoading: false });
            } else {
              set({ isLoading: false });
            }
            return;
          }
          // Network failure, 5xx or 429: the token has not been proven invalid, so keep
          // it. A backend outage must not sign the user out. Continue with the
          // persisted user; a genuinely expired token surfaces as a 401 on the next
          // request and is handled by sessionExpired.
          set({ isAuthenticated: get().user !== null, isLoading: false });
        }
      },

      sessionExpired: (rejectedToken) => {
        // A 401 for an older token (a request that raced a sign-in or an earlier
        // expiry) must not sign out the current session.
        if (get().token !== rejectedToken) return;
        api.clearToken();
        resetSessionContext();
        set({ token: null, user: null, isAuthenticated: false, isLoading: false });
        useToastStore.getState().warning('Session expired', 'Please sign in again.');
      },

      isPlatformAdmin: () => {
        const { user } = get();
        return user?.role === 'PlatformAdmin';
      },
    }),
    {
      name: 'ab-bot-auth',
      storage: createJSONStorage(() => localStorage),
      partialize: (state) => ({
        token: state.token,
        user: state.user,
      }),
      onRehydrateStorage: () => (state) => {
        if (state?.token) {
          api.setToken(state.token);
        }
        state?.setHasHydrated(true);
      },
    }
  )
);
