import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export type Theme = 'light' | 'dark' | 'system';

export interface AppearanceSettings {
  theme: Theme;
}

interface SettingsState {
  appearance: AppearanceSettings;
  updateAppearance: (updates: Partial<AppearanceSettings>) => void;
}

const defaultAppearance: AppearanceSettings = {
  theme: 'dark',
};

export const useSettingsStore = create<SettingsState>()(
  persist(
    (set) => ({
      appearance: defaultAppearance,

      updateAppearance: (updates) =>
        set((state) => ({
          appearance: { ...state.appearance, ...updates },
        })),
    }),
    {
      name: 'ab-bot-settings',
      partialize: (state) => ({
        appearance: state.appearance,
      }),
    }
  )
);
