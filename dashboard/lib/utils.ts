import { type ClassValue, clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatCurrency(
  value: number,
  options?: { showSign?: boolean; decimals?: number }
): string {
  const { showSign = false, decimals = 2 } = options ?? {};
  const formatted = new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
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
  options?: { showSign?: boolean; decimals?: number }
): string {
  const { showSign = false, decimals = 1 } = options ?? {};
  const formatted = `${Math.abs(value).toFixed(decimals)}%`;

  if (showSign && value !== 0) {
    return value > 0 ? `+${formatted}` : `-${formatted}`;
  }
  return value < 0 ? `-${formatted}` : formatted;
}

// Normalizes values that may be stored as either ratio (0.55) or percent (55).
export function ratioOrPercentToPercent(value: number | null | undefined): number {
  if (value === null || value === undefined || Number.isNaN(value)) {
    return 0;
  }
  return Math.abs(value) <= 1 ? value * 100 : value;
}

// Normalizes values that may be stored as either percent (55) or ratio (0.55).
export function ratioOrPercentToRatio(value: number | null | undefined): number {
  if (value === null || value === undefined || Number.isNaN(value)) {
    return 0;
  }
  return Math.abs(value) > 1 ? value / 100 : value;
}

export function formatNumber(value: number, decimals: number = 2): string {
  return new Intl.NumberFormat('en-US', {
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
  const d = typeof date === 'string' ? new Date(date) : date;
  const diffMs = now.getTime() - d.getTime();
  const diffSec = Math.floor(diffMs / 1000);
  const diffMin = Math.floor(diffSec / 60);
  const diffHour = Math.floor(diffMin / 60);
  const diffDay = Math.floor(diffHour / 24);

  if (diffSec < 60) return 'just now';
  if (diffMin < 60) return `${diffMin}m ago`;
  if (diffHour < 24) return `${diffHour}h ago`;
  if (diffDay < 7) return `${diffDay}d ago`;
  return d.toLocaleDateString();
}

export function calculatePnL(
  entryPrice: number,
  currentPrice: number,
  quantity: number,
  side: 'YES' | 'NO'
): { amount: number; percent: number } {
  const priceDiff = side === 'YES'
    ? currentPrice - entryPrice
    : entryPrice - currentPrice;
  const amount = priceDiff * quantity;
  const percent = (priceDiff / entryPrice) * 100;
  return { amount, percent };
}
