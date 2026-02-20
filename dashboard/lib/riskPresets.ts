export type RiskPreset = "conservative" | "balanced" | "aggressive";

export interface RiskPresetConfig {
  label: string;
  description: string;
  settings: {
    optimization_interval_hours: number;
    min_roi_30d: number;
    min_sharpe: number;
    min_win_rate: number;
    min_trades_30d: number;
  };
}

export const RISK_PRESETS: Record<RiskPreset, RiskPresetConfig> = {
  conservative: {
    label: "Conservative",
    description: "Higher quality bar, fewer rotations.",
    settings: {
      optimization_interval_hours: 24,
      min_roi_30d: 8,
      min_sharpe: 1.3,
      min_win_rate: 58,
      min_trades_30d: 20,
    },
  },
  balanced: {
    label: "Balanced",
    description: "Moderate quality bar for steady discovery.",
    settings: {
      optimization_interval_hours: 12,
      min_roi_30d: 5,
      min_sharpe: 1,
      min_win_rate: 50,
      min_trades_30d: 10,
    },
  },
  aggressive: {
    label: "Aggressive",
    description: "Lower thresholds for broader candidate exploration.",
    settings: {
      optimization_interval_hours: 6,
      min_roi_30d: 2,
      min_sharpe: 0.6,
      min_win_rate: 45,
      min_trades_30d: 5,
    },
  },
};
