import { create } from 'zustand';
import type { AlertBanner, AlertBannerType } from '@/components/ui/alert-banner';

interface OptimizationThresholds {
  min_roi_30d?: number;
  min_sharpe?: number;
  min_win_rate?: number;
  min_trades_30d?: number;
}

interface NotificationStore {
  // Alert banners (persistent until dismissed)
  banners: AlertBanner[];
  addBanner: (banner: Omit<AlertBanner, 'id'>) => string;
  removeBanner: (id: string) => void;
  clearBanners: () => void;

  // Convenience methods for automation events
  circuitBreakerTripped: (walletAddress: string, reason: string) => void;
  walletDemoted: (walletAddress: string, reason: string) => void;
  walletPromoted: (walletAddress: string) => void;
  probationGraduated: (walletAddress: string) => void;
  noWalletsFound: (thresholds: OptimizationThresholds, onAdjustThresholds?: () => void) => void;
  optimizationSuccess: (walletsPromoted: number) => void;
}

export const useNotificationStore = create<NotificationStore>((set, get) => ({
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

  // Automation event handlers that create persistent banners
  circuitBreakerTripped: (walletAddress, reason) => {
    const shortAddress = `${walletAddress.slice(0, 6)}...${walletAddress.slice(-4)}`;
    get().addBanner({
      type: 'error',
      title: `Trading paused for ${shortAddress}`,
      description: `Circuit breaker tripped: ${reason}`,
      dismissible: true,
      action: {
        label: 'View Details',
        onClick: () => {
          window.location.href = `/trading?tab=automation`;
        },
      },
    });
  },

  walletDemoted: (walletAddress, reason) => {
    const shortAddress = `${walletAddress.slice(0, 6)}...${walletAddress.slice(-4)}`;
    get().addBanner({
      type: 'warning',
      title: `${shortAddress} demoted`,
      description: reason,
      dismissible: true,
    });
  },

  walletPromoted: (walletAddress) => {
    const shortAddress = `${walletAddress.slice(0, 6)}...${walletAddress.slice(-4)}`;
    get().addBanner({
      type: 'success',
      title: `${shortAddress} promoted to Active`,
      description: 'Wallet has been auto-selected based on performance',
      dismissible: true,
    });
  },

  probationGraduated: (walletAddress) => {
    const shortAddress = `${walletAddress.slice(0, 6)}...${walletAddress.slice(-4)}`;
    get().addBanner({
      type: 'success',
      title: `${shortAddress} graduated`,
      description: 'Probation period complete - full allocation enabled',
      dismissible: true,
    });
  },

  noWalletsFound: (thresholds, onAdjustThresholds) => {
    const thresholdParts = [];
    if (thresholds.min_roi_30d !== undefined) {
      thresholdParts.push(`ROI > ${thresholds.min_roi_30d}%`);
    }
    if (thresholds.min_win_rate !== undefined) {
      thresholdParts.push(`Win Rate > ${thresholds.min_win_rate}%`);
    }
    if (thresholds.min_sharpe !== undefined) {
      thresholdParts.push(`Sharpe > ${thresholds.min_sharpe}`);
    }
    if (thresholds.min_trades_30d !== undefined) {
      thresholdParts.push(`Trades > ${thresholds.min_trades_30d}`);
    }
    const thresholdStr = thresholdParts.length > 0
      ? `Current thresholds: ${thresholdParts.join(', ')}`
      : 'Consider adjusting your thresholds';

    get().addBanner({
      type: 'warning',
      title: 'No wallets meet current thresholds',
      description: thresholdStr,
      dismissible: true,
      action: onAdjustThresholds ? {
        label: 'Adjust Thresholds',
        onClick: onAdjustThresholds,
      } : undefined,
    });
  },

  optimizationSuccess: (walletsPromoted) => {
    get().addBanner({
      type: 'success',
      title: `Optimization complete`,
      description: `${walletsPromoted} wallet${walletsPromoted !== 1 ? 's' : ''} promoted to active roster`,
      dismissible: true,
    });
  },
}));
