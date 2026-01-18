import { create } from 'zustand';
import { persist, createJSONStorage } from 'zustand/middleware';
import type { Workspace, WorkspaceListItem } from '@/types/api';
import api from '@/lib/api';

interface WorkspaceStore {
  workspaces: WorkspaceListItem[];
  currentWorkspace: Workspace | null;
  isLoading: boolean;
  error: string | null;
  _hasHydrated: boolean;

  setHasHydrated: (state: boolean) => void;
  fetchWorkspaces: () => Promise<void>;
  fetchCurrentWorkspace: () => Promise<void>;
  switchWorkspace: (workspaceId: string) => Promise<void>;
  setCurrentWorkspace: (workspace: Workspace | null) => void;
  reset: () => void;
}

export const useWorkspaceStore = create<WorkspaceStore>()(
  persist(
    (set, get) => ({
      workspaces: [],
      currentWorkspace: null,
      isLoading: false,
      error: null,
      _hasHydrated: false,

      setHasHydrated: (state) => {
        set({ _hasHydrated: state });
      },

      fetchWorkspaces: async () => {
        set({ isLoading: true, error: null });
        try {
          const workspaces = await api.listWorkspaces();
          set({ workspaces, isLoading: false });
        } catch (err) {
          set({
            error: err instanceof Error ? err.message : 'Failed to fetch workspaces',
            isLoading: false,
          });
        }
      },

      fetchCurrentWorkspace: async () => {
        set({ isLoading: true, error: null });
        try {
          const workspace = await api.getCurrentWorkspace();
          set({ currentWorkspace: workspace, isLoading: false });
        } catch (err) {
          // If 404, user has no workspace set
          if (err instanceof Error && err.message.includes('No workspace set')) {
            set({ currentWorkspace: null, isLoading: false });
          } else {
            set({
              error: err instanceof Error ? err.message : 'Failed to fetch current workspace',
              isLoading: false,
            });
          }
        }
      },

      switchWorkspace: async (workspaceId: string) => {
        set({ isLoading: true, error: null });
        try {
          await api.switchWorkspace(workspaceId);
          // Fetch the new current workspace details
          const workspace = await api.getWorkspace(workspaceId);
          set({ currentWorkspace: workspace, isLoading: false });
        } catch (err) {
          set({
            error: err instanceof Error ? err.message : 'Failed to switch workspace',
            isLoading: false,
          });
          throw err;
        }
      },

      setCurrentWorkspace: (workspace) => {
        set({ currentWorkspace: workspace });
      },

      reset: () => {
        set({
          workspaces: [],
          currentWorkspace: null,
          isLoading: false,
          error: null,
        });
      },
    }),
    {
      name: 'ab-bot-workspace',
      storage: createJSONStorage(() => localStorage),
      partialize: (state) => ({
        currentWorkspace: state.currentWorkspace,
      }),
      onRehydrateStorage: () => (state) => {
        state?.setHasHydrated(true);
      },
    }
  )
);
