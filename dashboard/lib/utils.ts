import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatCurrency(
  value: number,
  options?: { showSign?: boolean; decimals?: number },
): string {
  const { showSign = false, decimals = 2 } = options ?? {};
  const formatted = new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
  }).format(Math.abs(value));

  if (showSign && value !== 0) {
    return value > 0 ? `+${formatted}` : `-${formatted}`;
  }
  return value < 0 ? `-${formatted}` : formatted;
}

export function formatPercent(
  value: number,
  options?: { showSign?: boolean; decimals?: number },
): string {
  const { showSign = false, decimals = 1 } = options ?? {};
  const formatted = `${Math.abs(value).toFixed(decimals)}%`;

  if (showSign && value !== 0) {
    return value > 0 ? `+${formatted}` : `-${formatted}`;
  }
  return value < 0 ? `-${formatted}` : formatted;
}

// Normalizes values that may be stored as either ratio (0.55) or percent (55).
export function ratioOrPercentToPercent(
  value: number | null | undefined,
): number {
  if (value === null || value === undefined || Number.isNaN(value)) {
    return 0;
  }
  return Math.abs(value) <= 1 ? value * 100 : value;
}

// Normalizes values that may be stored as either percent (55) or ratio (0.55).
export function ratioOrPercentToRatio(
  value: number | null | undefined,
): number {
  if (value === null || value === undefined || Number.isNaN(value)) {
    return 0;
  }
  return Math.abs(value) > 1 ? value / 100 : value;
}

export function formatLargePercent(
  value: number,
  options?: { showSign?: boolean; decimals?: number },
): string {
  const { showSign = false, decimals = 1 } = options ?? {};
  const abs = Math.abs(value);
  const sign = showSign && value > 0 ? "+" : value < 0 ? "-" : "";

  if (abs >= 100000) {
    return `${sign}${(abs / 100).toFixed(0)}x`;
  }
  if (abs >= 10000) {
    return `${sign}${(abs / 1000).toFixed(1)}k%`;
  }
  if (abs >= 1000) {
    return `${sign}${(abs / 1000).toFixed(1)}k%`;
  }
  return `${sign}${abs.toFixed(decimals)}%`;
}

export function formatNumber(value: number, decimals: number = 2): string {
  return new Intl.NumberFormat("en-US", {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
  }).format(value);
}

export function shortenAddress(address: string, chars: number = 4): string {
  if (address.length <= chars * 2 + 2) return address;
  return `${address.slice(0, chars + 2)}...${address.slice(-chars)}`;
}

export function formatTimeAgo(date: Date | string): string {
  const now = new Date();
  const d = typeof date === "string" ? new Date(date) : date;
  const diffMs = now.getTime() - d.getTime();
  const diffSec = Math.floor(diffMs / 1000);
  const diffMin = Math.floor(diffSec / 60);
  const diffHour = Math.floor(diffMin / 60);
  const diffDay = Math.floor(diffHour / 24);

  if (diffSec < 60) return "just now";
  if (diffMin < 60) return `${diffMin}m ago`;
  if (diffHour < 24) return `${diffHour}h ago`;
  if (diffDay < 7) return `${diffDay}d ago`;
  return d.toLocaleDateString();
}

export function calculatePnL(
  entryPrice: number,
  currentPrice: number,
  quantity: number,
  side: "YES" | "NO",
): { amount: number; percent: number } {
  const priceDiff =
    side === "YES" ? currentPrice - entryPrice : entryPrice - currentPrice;
  const amount = priceDiff * quantity;
  const percent = (priceDiff / entryPrice) * 100;
  return { amount, percent };
}

export function formatDynamicKey(key: string | null): string {
  if (!key) return "(global)";
  const labels: Record<string, string> = {
    ARB_MONITOR_AGGRESSIVENESS_LEVEL: "Opportunity Aggressiveness",
    ARB_MONITOR_EXPLORATION_SLOTS: "Exploration Slots",
    ARB_MONITOR_MAX_MARKETS: "Max Monitored Markets",
    ARB_MIN_PROFIT_THRESHOLD: "Min Net Profit Threshold",
    COPY_MIN_TRADE_VALUE: "Min Copy Trade Value",
    COPY_MAX_SLIPPAGE_PCT: "Max Copy Slippage",
    COPY_MAX_LATENCY_SECS: "Max Copy Trade Age",
  };
  return labels[key] ?? key;
}

export function formatDynamicConfigValue(
  key: string | null,
  value: number | null,
): string {
  if (value === null) return "-";
  if (key === "ARB_MONITOR_AGGRESSIVENESS_LEVEL") {
    if (value <= 0.5) return "stable";
    if (value >= 1.5) return "discovery";
    return "balanced";
  }
  if (key === "COPY_MAX_LATENCY_SECS") {
    const secs = Math.round(value);
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return m > 0 ? `${m}m ${s}s` : `${s}s`;
  }
  if (key === "COPY_MIN_TRADE_VALUE") return formatCurrency(value);
  // Backend stores slippage as ratio (0.01 = 1%), so multiply by 100
  if (key === "COPY_MAX_SLIPPAGE_PCT") return `${(value * 100).toFixed(2)}%`;
  return Number.isInteger(value) ? String(value) : value.toFixed(4);
}
