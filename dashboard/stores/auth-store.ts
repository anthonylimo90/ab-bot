import { create } from 'zustand';
import { persist, createJSONStorage } from 'zustand/middleware';
import type { User } from '@/types/api';
import api from '@/lib/api';

interface AuthStore {
  token: string | null;
  user: User | null;
  isLoading: boolean;
  isAuthenticated: boolean;
  _hasHydrated: boolean;

  setAuth: (token: string, user: User) => void;
  logout: () => void;
  checkAuth: () => Promise<void>;
  setHasHydrated: (state: boolean) => void;
  isPlatformAdmin: () => boolean;
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
        api.setToken(token);
        set({ token, user, isAuthenticated: true, isLoading: false });
      },

      logout: () => {
        api.clearToken();
        set({ token: null, user: null, isAuthenticated: false, isLoading: false });
      },

      checkAuth: async () => {
        const { token } = get();
        if (!token) {
          set({ isLoading: false, isAuthenticated: false });
          return;
        }

        set({ isLoading: true });

        // Set token on API client
        api.setToken(token);

        try {
          const user = await api.getCurrentUser();
          set({ user, isAuthenticated: true, isLoading: false });
        } catch {
          // Token invalid or expired
          api.clearToken();
          set({ token: null, user: null, isAuthenticated: false, isLoading: false });
        }
      },

      isPlatformAdmin: () => {
        const { user } = get();
        return user?.role === 'Admin';
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
        state?.setHasHydrated(true);
      },
    }
  )
);
