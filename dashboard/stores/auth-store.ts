import { create } from 'zustand';
import { persist, createJSONStorage } from 'zustand/middleware';
import type { User } from '@/types/api';
import api, { ApiHttpError } from '@/lib/api';
import { useToastStore } from '@/stores/toast-store';
import { queryClient } from '@/lib/queryClient';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { useWalletStore } from '@/stores/wallet-store';

// The session itself is an HttpOnly cookie the API sets: no script can read
// it. This store keeps only who is signed in, to render the page while the
// session is being checked.
interface AuthStore {
  user: User | null;
  isLoading: boolean;
  isAuthenticated: boolean;
  _hasHydrated: boolean;

  /** After a sign-in, whose session the API has put in the cookie. */
  setAuth: (user: User) => void;
  logout: () => Promise<boolean>;
  checkAuth: () => Promise<void>;
  sessionExpired: (rejectedSession: number) => void;
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
      user: null,
      isLoading: false,
      isAuthenticated: false,
      _hasHydrated: false,

      setHasHydrated: (state) => {
        set({ _hasHydrated: state });
      },

      setAuth: (user) => {
        api.beginSession();
        resetSessionContext();
        set({ user, isAuthenticated: true, isLoading: false });
      },

      logout: async () => {
        const session = api.currentSession();
        try {
          await api.logout();
        } catch (error) {
          if (!(error instanceof ApiHttpError && error.isUnauthorized)) {
            useToastStore.getState().error('Logout failed', 'Your session is still active. Please try again.');
            return false;
          }
        }
        // A concurrent sign-in must not be cleared by an older logout request.
        if (api.currentSession() !== session) return false;
        api.beginSession();
        resetSessionContext();
        set({ user: null, isAuthenticated: false, isLoading: false });
        return true;
      },

      checkAuth: async () => {
        const session = api.currentSession();
        set({ isLoading: true });
        try {
          const user = await api.getCurrentUser();
          set({ user, isAuthenticated: true, isLoading: false });
        } catch (error) {
          if (error instanceof ApiHttpError && error.isUnauthorized) {
            // No session, or one expired or revoked. A concurrent sign-in
            // must not be cleared by this older check.
            if (api.currentSession() === session) {
              resetSessionContext();
              set({ user: null, isAuthenticated: false, isLoading: false });
            } else {
              set({ isLoading: false });
            }
            return;
          }
          // Network failure, 5xx or 429: the session has not been proven
          // invalid. A backend outage must not sign the user out: continue
          // with the remembered user; a genuinely expired session surfaces as
          // a 401 on the next request and is handled by sessionExpired.
          set({ isAuthenticated: get().user !== null, isLoading: false });
        }
      },

      sessionExpired: (rejectedSession) => {
        // A 401 for a request sent in an earlier session (one that raced a
        // sign-in, or an earlier expiry) must not sign out the current one.
        if (api.currentSession() !== rejectedSession || !get().isAuthenticated) return;
        api.beginSession();
        resetSessionContext();
        set({ user: null, isAuthenticated: false, isLoading: false });
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
      // Who is signed in only; never a token. (Earlier versions stored the
      // token here: the next write replaces the stored state without it.)
      partialize: (state) => ({
        user: state.user,
      }),
      onRehydrateStorage: () => (state) => {
        state?.setHasHydrated(true);
      },
    }
  )
);
