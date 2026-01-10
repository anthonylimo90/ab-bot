import { z } from 'zod';

// Auth page schemas
export const loginSchema = z.object({
  email: z.string().email('Invalid email address'),
  password: z.string().min(1, 'Password is required'),
});

export type LoginFormData = z.infer<typeof loginSchema>;

export const signupSchema = z
  .object({
    email: z.string().email('Invalid email address'),
    password: z.string().min(8, 'Password must be at least 8 characters'),
    confirmPassword: z.string(),
    name: z.string().optional(),
  })
  .refine((data) => data.password === data.confirmPassword, {
    message: "Passwords don't match",
    path: ['confirmPassword'],
  });

export type SignupFormData = z.infer<typeof signupSchema>;

// Allocation page schemas
export const allocationItemSchema = z.object({
  id: z.string(),
  percent: z.number().min(0, 'Allocation must be at least 0%').max(100, 'Allocation cannot exceed 100%'),
});

export const allocateSchema = z.object({
  budget: z
    .number({ required_error: 'Budget is required', invalid_type_error: 'Budget must be a number' })
    .min(1, 'Budget must be at least $1')
    .max(1000000, 'Budget cannot exceed $1,000,000'),
  strategy: z.enum(['EQUAL_WEIGHT', 'PERFORMANCE_WEIGHTED', 'RISK_ADJUSTED', 'CUSTOM'], {
    required_error: 'Please select an allocation strategy',
  }),
  allocations: z
    .array(allocationItemSchema)
    .min(1, 'Select at least one strategy')
    .max(5, 'Maximum 5 strategies allowed')
    .refine(
      (arr) => {
        const total = arr.reduce((sum, item) => sum + item.percent, 0);
        return Math.abs(total - 100) < 0.01; // Allow small floating point errors
      },
      { message: 'Allocations must sum to 100%' }
    ),
});

export type AllocateFormData = z.infer<typeof allocateSchema>;

// Backtest page schemas
export const backtestSchema = z
  .object({
    strategyType: z.enum(['Arbitrage', 'Momentum', 'MeanReversion', 'CopyTrading'], {
      required_error: 'Please select a strategy type',
    }),
    startDate: z.date({ required_error: 'Start date is required' }),
    endDate: z.date({ required_error: 'End date is required' }),
    initialCapital: z
      .number({ required_error: 'Initial capital is required', invalid_type_error: 'Must be a number' })
      .min(100, 'Minimum capital is $100')
      .max(10000000, 'Maximum capital is $10,000,000'),
    // Strategy-specific params
    minSpread: z.number().min(0).max(100).optional(),
    maxPosition: z.number().min(1).optional(),
    lookbackHours: z.number().min(1).max(720).optional(),
    threshold: z.number().min(0).max(100).optional(),
    windowHours: z.number().min(1).max(720).optional(),
    stdThreshold: z.number().min(0).max(10).optional(),
    slippageModel: z.enum(['None', 'Fixed', 'VolumeBased']).optional(),
    slippagePct: z.number().min(0).max(10).optional(),
    feePct: z.number().min(0).max(10).optional(),
  })
  .refine((data) => data.endDate > data.startDate, {
    message: 'End date must be after start date',
    path: ['endDate'],
  });

export type BacktestFormData = z.infer<typeof backtestSchema>;

// Copy wallet modal schemas
export const copyWalletSchema = z.object({
  allocationPct: z
    .number({ required_error: 'Allocation is required', invalid_type_error: 'Must be a number' })
    .min(1, 'Allocation must be at least 1%')
    .max(100, 'Allocation cannot exceed 100%'),
  maxPositionSize: z
    .number({ required_error: 'Max position size is required', invalid_type_error: 'Must be a number' })
    .min(1, 'Minimum position size is $1')
    .max(100000, 'Maximum position size is $100,000'),
  copyBehavior: z.enum(['copy_all', 'events_only', 'arb_threshold'], {
    required_error: 'Please select a copy behavior',
  }),
  arbThresholdPct: z.number().min(0).max(50).optional(),
});

export type CopyWalletFormData = z.infer<typeof copyWalletSchema>;

// Discover page filter schemas
export const discoverFilterSchema = z.object({
  minRoi: z.number().min(-100).max(1000).optional(),
  minWinRate: z.number().min(0).max(100).optional(),
  minTrades: z.number().min(0).max(10000).optional(),
  sortBy: z.enum(['roi', 'sharpe', 'winRate', 'trades']).optional(),
  period: z.enum(['7d', '30d', '90d']).optional(),
  hideBots: z.boolean().optional(),
});

export type DiscoverFilterFormData = z.infer<typeof discoverFilterSchema>;

// Settings page schemas
export const settingsSchema = z.object({
  demoBalance: z
    .number({ required_error: 'Balance is required' })
    .min(100, 'Minimum balance is $100')
    .max(10000000, 'Maximum balance is $10,000,000'),
  autoRebalance: z.boolean(),
  rebalanceThreshold: z.number().min(1).max(50).optional(),
  notifications: z.object({
    tradeAlerts: z.boolean(),
    priceAlerts: z.boolean(),
    emailDigest: z.boolean(),
  }),
});

export type SettingsFormData = z.infer<typeof settingsSchema>;

// Helper to format Zod errors for display
export function formatZodError(error: z.ZodError): Record<string, string> {
  const errors: Record<string, string> = {};
  for (const issue of error.issues) {
    const path = issue.path.join('.');
    if (!errors[path]) {
      errors[path] = issue.message;
    }
  }
  return errors;
}
