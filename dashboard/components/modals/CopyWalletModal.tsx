"use client";

import { useEffect } from "react";
import { useForm, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Slider } from "@/components/ui/slider";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Wallet, Users, AlertTriangle, Zap } from "lucide-react";
import { shortenAddress, formatCurrency } from "@/lib/utils";
import { copyWalletSchema, type CopyWalletFormData } from "@/lib/validations";
import type { CopyBehavior } from "@/types/api";

interface WalletInfo {
  address: string;
  roi30d?: number;
  sharpe?: number;
  winRate?: number;
  trades?: number;
  confidence?: number;
}

interface CopyWalletModalProps {
  wallet: WalletInfo | null;
  isOpen: boolean;
  onClose: () => void;
  onConfirm: (settings: {
    address: string;
    allocation_pct: number;
    copy_behavior: CopyBehavior;
    max_position_size: number;
    tier: "active" | "bench";
  }) => void;
  rosterCount: number;
  maxRoster?: number;
}

const copyBehaviorLabels: Record<
  CopyBehavior,
  { label: string; description: string }
> = {
  copy_all: {
    label: "Copy All Trades",
    description: "Mirror all trades from this wallet",
  },
  events_only: {
    label: "Events Only",
    description: "Only copy directional event trades, skip arbitrage",
  },
  arb_threshold: {
    label: "Arb Threshold",
    description: "Replicate arb logic only when spread exceeds threshold",
  },
};

export function CopyWalletModal({
  wallet,
  isOpen,
  onClose,
  onConfirm,
  rosterCount,
  maxRoster = 5,
}: CopyWalletModalProps) {
  const canAddToActive = rosterCount < maxRoster;
  const slotsRemaining = maxRoster - rosterCount;

  const {
    control,
    handleSubmit,
    watch,
    setValue,
    reset,
    formState: { errors, isValid },
  } = useForm<CopyWalletFormData & { tier: "active" | "bench" }>({
    resolver: zodResolver(copyWalletSchema),
    defaultValues: {
      allocationPct: 10,
      maxPositionSize: 100,
      copyBehavior: "events_only",
      tier: canAddToActive ? "active" : "bench",
    },
    mode: "onChange",
  });

  const tier = watch("tier");
  const copyBehavior = watch("copyBehavior");

  // Reset form when modal opens/closes or wallet changes
  useEffect(() => {
    if (isOpen) {
      reset({
        allocationPct: 10,
        maxPositionSize: 100,
        copyBehavior: "events_only",
        tier: canAddToActive ? "active" : "bench",
      });
    }
  }, [isOpen, wallet?.address, canAddToActive, reset]);

  const onSubmit = (
    data: CopyWalletFormData & { tier: "active" | "bench" },
  ) => {
    if (!wallet) return;
    onConfirm({
      address: wallet.address,
      allocation_pct: data.allocationPct,
      copy_behavior: data.copyBehavior,
      max_position_size: data.maxPositionSize,
      tier: data.tier,
    });
    onClose();
  };

  if (!wallet) return null;

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="w-[calc(100vw-2rem)] sm:max-w-[500px]">
        <form onSubmit={handleSubmit(onSubmit)}>
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <Wallet className="h-5 w-5" />
              Copy Wallet
            </DialogTitle>
            <DialogDescription>
              Configure copy trading settings for this wallet
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-6 py-4">
            {/* Wallet Info */}
            <div className="rounded-lg border p-4 space-y-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="font-mono font-medium">
                  {shortenAddress(wallet.address)}
                </span>
                {wallet.confidence && (
                  <span className="text-xs px-2 py-1 rounded-full bg-muted text-muted-foreground">
                    {wallet.confidence}% confidence
                  </span>
                )}
              </div>
              <div className="grid grid-cols-2 gap-2 text-sm sm:grid-cols-4">
                {wallet.roi30d !== undefined && (
                  <div>
                    <p className="text-xs text-muted-foreground">ROI</p>
                    <p className="font-medium text-profit">+{wallet.roi30d}%</p>
                  </div>
                )}
                {wallet.sharpe !== undefined && (
                  <div>
                    <p className="text-xs text-muted-foreground">Sharpe</p>
                    <p className="font-medium">{wallet.sharpe}</p>
                  </div>
                )}
                {wallet.winRate !== undefined && (
                  <div>
                    <p className="text-xs text-muted-foreground">Win Rate</p>
                    <p className="font-medium">{wallet.winRate}%</p>
                  </div>
                )}
                {wallet.trades !== undefined && (
                  <div>
                    <p className="text-xs text-muted-foreground">Trades</p>
                    <p className="font-medium">{wallet.trades}</p>
                  </div>
                )}
              </div>
            </div>

            {/* Tier Selection */}
            <div className="space-y-3">
              <Label className="flex items-center gap-2">
                <Users className="h-4 w-4" />
                Add to
              </Label>
              <Controller
                name="tier"
                control={control}
                render={({ field }) => (
                  <div className="grid grid-cols-2 gap-3">
                    <button
                      type="button"
                      onClick={() => canAddToActive && field.onChange("active")}
                      disabled={!canAddToActive}
                      className={`p-3 rounded-lg border text-left transition-colors ${
                        field.value === "active"
                          ? "border-primary bg-primary/5"
                          : "border-border hover:border-muted-foreground"
                      } ${!canAddToActive ? "opacity-50 cursor-not-allowed" : ""}`}
                    >
                      <div className="font-medium">Active</div>
                      <div className="text-xs text-muted-foreground">
                        {canAddToActive
                          ? `${slotsRemaining} slot${slotsRemaining !== 1 ? "s" : ""} available`
                          : "Roster full"}
                      </div>
                    </button>
                    <button
                      type="button"
                      onClick={() => field.onChange("bench")}
                      className={`p-3 rounded-lg border text-left transition-colors ${
                        field.value === "bench"
                          ? "border-primary bg-primary/5"
                          : "border-border hover:border-muted-foreground"
                      }`}
                    >
                      <div className="font-medium">Watching</div>
                      <div className="text-xs text-muted-foreground">
                        Monitor & evaluate
                      </div>
                    </button>
                  </div>
                )}
              />
              {!canAddToActive && tier === "bench" && (
                <p className="text-xs text-muted-foreground flex items-center gap-1">
                  <AlertTriangle className="h-3 w-3" />
                  Active roster is full. Demote a wallet first to add to Active.
                </p>
              )}
            </div>

            {/* Copy Behavior */}
            <div className="space-y-3">
              <Label className="flex items-center gap-2">
                <Zap className="h-4 w-4" />
                Copy Behavior
              </Label>
              <Controller
                name="copyBehavior"
                control={control}
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {Object.entries(copyBehaviorLabels).map(
                        ([key, { label, description }]) => (
                          <SelectItem key={key} value={key}>
                            <div>
                              <div className="font-medium">{label}</div>
                              <div className="text-xs text-muted-foreground">
                                {description}
                              </div>
                            </div>
                          </SelectItem>
                        ),
                      )}
                    </SelectContent>
                  </Select>
                )}
              />
              {errors.copyBehavior && (
                <p className="text-sm text-destructive">
                  {errors.copyBehavior.message}
                </p>
              )}
            </div>

            {/* Arb Threshold (shown when arb_threshold selected) */}
            {copyBehavior === "arb_threshold" && (
              <div className="space-y-3">
                <Label htmlFor="arbThreshold">Arb Threshold (%)</Label>
                <Controller
                  name="arbThresholdPct"
                  control={control}
                  render={({ field }) => (
                    <Input
                      id="arbThreshold"
                      type="number"
                      min={0}
                      max={50}
                      step={0.5}
                      placeholder="2.0"
                      value={field.value ?? ""}
                      onChange={(e) =>
                        field.onChange(e.target.valueAsNumber || undefined)
                      }
                      error={!!errors.arbThresholdPct}
                    />
                  )}
                />
                <p className="text-xs text-muted-foreground">
                  Minimum spread percentage to replicate arbitrage trades
                </p>
                {errors.arbThresholdPct && (
                  <p className="text-sm text-destructive">
                    {errors.arbThresholdPct.message}
                  </p>
                )}
              </div>
            )}

            {/* Allocation */}
            {tier === "active" && (
              <div className="space-y-3">
                <Controller
                  name="allocationPct"
                  control={control}
                  render={({ field }) => (
                    <>
                      <div className="flex items-center justify-between">
                        <Label>Allocation</Label>
                        <span className="text-sm font-medium">
                          {field.value}%
                        </span>
                      </div>
                      <Slider
                        value={[field.value]}
                        onValueChange={([v]) => field.onChange(v)}
                        min={1}
                        max={50}
                        step={1}
                      />
                      <p className="text-xs text-muted-foreground">
                        Percentage of your capital allocated to copying this
                        wallet
                      </p>
                      {errors.allocationPct && (
                        <p className="text-sm text-destructive">
                          {errors.allocationPct.message}
                        </p>
                      )}
                    </>
                  )}
                />
              </div>
            )}

            {/* Max Position */}
            <div className="space-y-3">
              <Controller
                name="maxPositionSize"
                control={control}
                render={({ field }) => (
                  <>
                    <div className="flex items-center justify-between">
                      <Label>Max Position Size</Label>
                      <span className="text-sm font-medium">
                        {formatCurrency(field.value)}
                      </span>
                    </div>
                    <Slider
                      value={[field.value]}
                      onValueChange={([v]) => field.onChange(v)}
                      min={10}
                      max={1000}
                      step={10}
                    />
                    <p className="text-xs text-muted-foreground">
                      Maximum size for any single copied position
                    </p>
                    {errors.maxPositionSize && (
                      <p className="text-sm text-destructive">
                        {errors.maxPositionSize.message}
                      </p>
                    )}
                  </>
                )}
              />
            </div>
          </div>

          <DialogFooter className="flex-col gap-2 sm:flex-row">
            <Button type="button" variant="outline" onClick={onClose} className="w-full sm:w-auto">
              Cancel
            </Button>
            <Button type="submit" className="w-full sm:w-auto">
              {tier === "active" ? "Add to Active" : "Add to Watching"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
