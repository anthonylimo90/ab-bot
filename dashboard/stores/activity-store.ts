import { create } from "zustand";

interface ActivityState {
  unreadCount: number;
  increment: () => void;
  reset: () => void;
}

export const useActivityStore = create<ActivityState>()((set) => ({
  unreadCount: 0,
  increment: () => set((state) => ({ unreadCount: state.unreadCount + 1 })),
  reset: () => set({ unreadCount: 0 }),
}));
