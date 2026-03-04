import { z } from "zod";

// Auth page schemas
export const loginSchema = z.object({
  email: z.string().email("Invalid email address"),
  password: z.string().min(1, "Password is required"),
});

export type LoginFormData = z.infer<typeof loginSchema>;

// Backtest page schemas
export const backtestSchema = z
  .object({
    strategyType: z.enum(
      ["Arbitrage", "Momentum", "MeanReversion"],
      {
        required_error: "Please select a strategy type",
      },
    ),
    startDate: z.date({ required_error: "Start date is required" }),
    endDate: z.date({ required_error: "End date is required" }),
    initialCapital: z
      .number({
        required_error: "Initial capital is required",
        invalid_type_error: "Must be a number",
      })
      .min(100, "Minimum capital is $100")
      .max(10000000, "Maximum capital is $10,000,000"),
    // Strategy-specific params
    minSpread: z.number().min(0).max(100).optional(),
    maxPosition: z.number().min(1).optional(),
    lookbackHours: z.number().min(1).max(720).optional(),
    threshold: z.number().min(0).max(100).optional(),
    windowHours: z.number().min(1).max(720).optional(),
    stdThreshold: z.number().min(0).max(10).optional(),
    slippageModel: z.enum(["None", "Fixed", "VolumeBased"]).optional(),
    slippagePct: z.number().min(0).max(10).optional(),
    feePct: z.number().min(0).max(10).optional(),
  })
  .refine((data) => data.endDate > data.startDate, {
    message: "End date must be after start date",
    path: ["endDate"],
  });

export type BacktestFormData = z.infer<typeof backtestSchema>;

// Settings page schemas
export const settingsSchema = z.object({
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
    const path = issue.path.join(".");
    if (!errors[path]) {
      errors[path] = issue.message;
    }
  }
  return errors;
}
