import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
import type { Workspace, WorkspaceListItem } from "@/types/api";
import api from "@/lib/api";

interface WorkspaceStore {
  workspaces: WorkspaceListItem[];
  currentWorkspace: Workspace | null;
  /** Counts every change of `currentWorkspace`: a read that started before one is stale. */
  workspaceVersion: number;
  isLoading: boolean;
  error: string | null;
  _hasHydrated: boolean;

  setHasHydrated: (state: boolean) => void;
  fetchWorkspaces: () => Promise<void>;
  fetchCurrentWorkspace: () => Promise<void>;
  refreshCurrentWorkspace: () => Promise<void>;
  switchWorkspace: (workspaceId: string) => Promise<void>;
  setCurrentWorkspace: (workspace: Workspace | null) => void;
  reset: () => void;
}

export const useWorkspaceStore = create<WorkspaceStore>()(
  persist(
    (set, get) => ({
      workspaces: [],
      currentWorkspace: null,
      workspaceVersion: 0,
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
            error:
              err instanceof Error ? err.message : "Failed to fetch workspaces",
            isLoading: false,
          });
        }
      },

      fetchCurrentWorkspace: async () => {
        set({ isLoading: true, error: null });
        try {
          const workspace = await api.getCurrentWorkspace();
          set((state) => ({
            currentWorkspace: workspace,
            workspaceVersion: state.workspaceVersion + 1,
            isLoading: false,
          }));
        } catch (err) {
          // If 404, user has no workspace set
          if (
            err instanceof Error &&
            err.message.includes("No workspace set")
          ) {
            set((state) => ({
              currentWorkspace: null,
              workspaceVersion: state.workspaceVersion + 1,
              isLoading: false,
            }));
          } else {
            set({
              error:
                err instanceof Error
                  ? err.message
                  : "Failed to fetch current workspace",
              isLoading: false,
            });
          }
        }
      },

      // Read the current workspace again, without the loading state, and
      // store it only when it changed: another owner's save shows here, and a
      // read that finds nothing new leaves the forms built from it alone.
      refreshCurrentWorkspace: async () => {
        const shown = get().currentWorkspace;
        if (!shown) return;
        const version = get().workspaceVersion;
        try {
          const workspace = await api.getWorkspace(shown.id);
          const current = get().currentWorkspace;
          // Unless the workspace changed meanwhile (switched, or a save
          // stored a newer copy than this read may have seen).
          if (
            get().workspaceVersion === version &&
            current &&
            current.id === workspace.id &&
            JSON.stringify(current) !== JSON.stringify(workspace)
          ) {
            set((state) => ({
              currentWorkspace: workspace,
              workspaceVersion: state.workspaceVersion + 1,
            }));
          }
        } catch {
          // The next read tries again.
        }
      },

      switchWorkspace: async (workspaceId: string) => {
        set({ isLoading: true, error: null });
        try {
          await api.switchWorkspace(workspaceId);
          // Fetch the new current workspace details
          const workspace = await api.getWorkspace(workspaceId);
          set((state) => ({
            currentWorkspace: workspace,
            workspaceVersion: state.workspaceVersion + 1,
            isLoading: false,
          }));
        } catch (err) {
          set({
            error:
              err instanceof Error ? err.message : "Failed to switch workspace",
            isLoading: false,
          });
          throw err;
        }
      },

      setCurrentWorkspace: (workspace) => {
        set((state) => ({
          currentWorkspace: workspace,
          workspaceVersion: state.workspaceVersion + 1,
        }));
      },

      reset: () => {
        set((state) => ({
          workspaces: [],
          currentWorkspace: null,
          workspaceVersion: state.workspaceVersion + 1,
          isLoading: false,
          error: null,
        }));
      },
    }),
    {
      name: "ab-bot-workspace",
      storage: createJSONStorage(() => localStorage),
      partialize: () => ({}),
      onRehydrateStorage: () => (state) => {
        state?.setHasHydrated(true);
      },
    },
  ),
);
