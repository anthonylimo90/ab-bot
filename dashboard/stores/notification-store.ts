import { create } from 'zustand';
import type { AlertBanner } from '@/components/ui/alert-banner';

interface NotificationStore {
  // Alert banners (persistent until dismissed)
  banners: AlertBanner[];
  addBanner: (banner: Omit<AlertBanner, 'id'>) => string;
  removeBanner: (id: string) => void;
  clearBanners: () => void;
}

export const useNotificationStore = create<NotificationStore>((set) => ({
  banners: [],

  addBanner: (banner) => {
    const id = Math.random().toString(36).slice(2);
    set((state) => ({
      banners: [
        ...state.banners,
        { ...banner, id },
      ],
    }));
    return id;
  },

  removeBanner: (id) => {
    set((state) => ({
      banners: state.banners.filter((b) => b.id !== id),
    }));
  },

  clearBanners: () => {
    set({ banners: [] });
  },
}));
