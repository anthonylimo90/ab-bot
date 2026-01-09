import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export interface RiskSettings {
  defaultStopLoss: number;
  maxPositionSize: number;
  circuitBreakerEnabled: boolean;
  dailyLossLimit: number;
}

export interface NotificationSettings {
  telegramEnabled: boolean;
  telegramChatId?: string;
  discordEnabled: boolean;
  discordWebhook?: string;
  emailEnabled: boolean;
  emailAddress?: string;
}

export type Theme = 'light' | 'dark' | 'system';

export interface AppearanceSettings {
  theme: Theme;
}

interface SettingsState {
  risk: RiskSettings;
  notifications: NotificationSettings;
  appearance: AppearanceSettings;

  // Track if there are unsaved changes
  isDirty: boolean;

  // Actions
  updateRisk: (updates: Partial<RiskSettings>) => void;
  updateNotifications: (updates: Partial<NotificationSettings>) => void;
  updateAppearance: (updates: Partial<AppearanceSettings>) => void;
  markClean: () => void;
  resetToDefaults: () => void;
}

const defaultRisk: RiskSettings = {
  defaultStopLoss: 15,
  maxPositionSize: 500,
  circuitBreakerEnabled: true,
  dailyLossLimit: 1000,
};

const defaultNotifications: NotificationSettings = {
  telegramEnabled: false,
  discordEnabled: false,
  emailEnabled: false,
};

const defaultAppearance: AppearanceSettings = {
  theme: 'dark',
};

export const useSettingsStore = create<SettingsState>()(
  persist(
    (set) => ({
      risk: defaultRisk,
      notifications: defaultNotifications,
      appearance: defaultAppearance,
      isDirty: false,

      updateRisk: (updates) =>
        set((state) => ({
          risk: { ...state.risk, ...updates },
          isDirty: true,
        })),

      updateNotifications: (updates) =>
        set((state) => ({
          notifications: { ...state.notifications, ...updates },
          isDirty: true,
        })),

      updateAppearance: (updates) =>
        set((state) => ({
          appearance: { ...state.appearance, ...updates },
          isDirty: true,
        })),

      markClean: () => set({ isDirty: false }),

      resetToDefaults: () =>
        set({
          risk: defaultRisk,
          notifications: defaultNotifications,
          appearance: defaultAppearance,
          isDirty: true,
        }),
    }),
    {
      name: 'ab-bot-settings',
      partialize: (state) => ({
        risk: state.risk,
        notifications: state.notifications,
        appearance: state.appearance,
      }),
    }
  )
);
