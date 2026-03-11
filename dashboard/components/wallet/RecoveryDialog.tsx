"use client";

import { useState } from "react";
import {
  AlertTriangle,
  LifeBuoy,
  Loader2,
  RefreshCw,
  ShieldAlert,
  Wallet,
} from "lucide-react";

import { useRecoveryPreviewQuery, useRunRecoveryMutation } from "@/hooks/queries/useRecoveryQuery";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { useToastStore } from "@/stores/toast-store";

const usdFormatter = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});

interface RecoveryDialogProps {
  workspaceId?: string | null;
  disabled?: boolean;
  compact?: boolean;
}

function RecoveryMetricCard({
  title,
  value,
  positions,
  tone = "default",
}: {
  title: string;
  value: number;
  positions: number;
  tone?: "default" | "warning" | "danger";
}) {
  return (
    <div
      className={cn(
        "rounded-2xl border px-4 py-3",
        tone === "danger"
          ? "border-rose-200 bg-rose-50/70"
          : tone === "warning"
            ? "border-amber-200 bg-amber-50/70"
            : "border-border/70 bg-card/70",
      )}
    >
      <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
        {title}
      </div>
      <div className="mt-1 text-lg font-semibold tabular-nums">
        {usdFormatter.format(value)}
      </div>
      <div className="mt-1 text-xs text-muted-foreground">
        {positions} position{positions === 1 ? "" : "s"}
      </div>
    </div>
  );
}

export function RecoveryDialog({
  workspaceId,
  disabled = false,
  compact = false,
}: RecoveryDialogProps) {
  const [open, setOpen] = useState(false);
  const toast = useToastStore();
  const previewQuery = useRecoveryPreviewQuery(workspaceId, open);
  const runMutation = useRunRecoveryMutation(workspaceId);

  const handleRunRecovery = async () => {
    if (!workspaceId) return;

    try {
      const result = await runMutation.mutateAsync();
      if (result.warnings.length > 0) {
        toast.warning("Recovery initiated with warnings", result.warnings[0]);
      } else {
        toast.success(
          "Recovery initiated",
          `Requeued ${result.safe_exit_failures_requeued} failed exits and reopened ${result.stalled_positions_reopened} stalled positions.`,
        );
      }
    } catch (error) {
      toast.error(
        "Recovery failed",
        error instanceof Error ? error.message : "Unable to start recovery",
      );
    }
  };

  const preview = previewQuery.data;
  const runResult = runMutation.data;
  const isUnavailable = disabled || !workspaceId;

  return (
    <>
      <Button
        type="button"
        variant="outline"
        size={compact ? "sm" : "default"}
        className={cn(
          "shrink-0 rounded-full",
          compact ? "h-8 gap-1.5 px-2.5 text-xs" : "h-9 gap-2 px-3 text-sm",
        )}
        disabled={isUnavailable}
        onClick={() => setOpen(true)}
      >
        <LifeBuoy className="h-4 w-4" />
        <span>{compact ? "Recover" : "Recover"}</span>
      </Button>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-[560px]">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <Wallet className="h-5 w-5 text-primary" />
              Safe Recovery
            </DialogTitle>
            <DialogDescription>
              Re-arm live exit processing and requeue non-suspect recovery buckets.
              Inventory mismatches stay excluded.
            </DialogDescription>
          </DialogHeader>

          {!workspaceId ? (
            <div className="rounded-xl border border-dashed px-4 py-6 text-sm text-muted-foreground">
              Select a workspace before starting recovery.
            </div>
          ) : previewQuery.isPending ? (
            <div className="flex min-h-40 items-center justify-center">
              <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
            </div>
          ) : previewQuery.isError || !preview ? (
            <div className="rounded-xl border border-rose-200 bg-rose-50/70 px-4 py-4 text-sm text-rose-700">
              {previewQuery.error instanceof Error
                ? previewQuery.error.message
                : "Unable to load recovery preview"}
            </div>
          ) : (
            <div className="space-y-4">
              {(!preview.live_running || !preview.exit_handler_running) && (
                <div className="flex items-start gap-3 rounded-2xl border border-amber-200 bg-amber-50/80 px-4 py-3 text-sm text-amber-900">
                  <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
                  <div>
                    <div className="font-medium">Runtime attention needed</div>
                    <div className="text-amber-800/90">
                      {!preview.live_running
                        ? "Live trading is not fully running."
                        : "Exit handler heartbeat is stale."}
                    </div>
                  </div>
                </div>
              )}

              <div className="grid gap-3 sm:grid-cols-2">
                <RecoveryMetricCard
                  title="Safe Recovery Candidate"
                  value={preview.safe_recovery.marked_value}
                  positions={preview.safe_recovery.positions}
                />
                <RecoveryMetricCard
                  title="Recoverable Now"
                  value={preview.recoverable_now.marked_value}
                  positions={preview.recoverable_now.positions}
                />
                <RecoveryMetricCard
                  title="Liquidity Blocked"
                  value={preview.liquidity_blocked.marked_value}
                  positions={preview.liquidity_blocked.positions}
                  tone="warning"
                />
                <RecoveryMetricCard
                  title="Suspect Inventory"
                  value={preview.suspect_inventory.marked_value}
                  positions={preview.suspect_inventory.positions}
                  tone="danger"
                />
              </div>

              <div className="grid gap-3 sm:grid-cols-3">
                <RecoveryMetricCard
                  title="Stalled"
                  value={preview.stalled.marked_value}
                  positions={preview.stalled.positions}
                  tone="warning"
                />
                <RecoveryMetricCard
                  title="Open Monitoring"
                  value={preview.open_monitoring.marked_value}
                  positions={preview.open_monitoring.positions}
                />
                <RecoveryMetricCard
                  title="Other Blocked"
                  value={preview.other_blocked.marked_value}
                  positions={preview.other_blocked.positions}
                />
              </div>

              {runResult && (
                <div className="rounded-2xl border border-emerald-200 bg-emerald-50/70 px-4 py-3 text-sm text-emerald-950">
                  <div className="flex items-center gap-2 font-medium">
                    <RefreshCw className="h-4 w-4" />
                    Recovery queued
                  </div>
                  <div className="mt-1 text-emerald-900/90">
                    Requeued {runResult.safe_exit_failures_requeued} safe exit failures and reopened{" "}
                    {runResult.stalled_positions_reopened} stalled positions.
                  </div>
                  {runResult.warnings.length > 0 && (
                    <div className="mt-2 flex items-start gap-2 text-amber-900">
                      <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0" />
                      <span>{runResult.warnings[0]}</span>
                    </div>
                  )}
                </div>
              )}
            </div>
          )}

          <DialogFooter className="gap-2 sm:justify-between">
            <div className="text-xs text-muted-foreground">
              Safe recovery excludes 404 and conditional-balance mismatches.
            </div>
            <Button
              type="button"
              onClick={handleRunRecovery}
              disabled={!workspaceId || previewQuery.isPending || runMutation.isPending}
            >
              {runMutation.isPending ? (
                <>
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  Starting…
                </>
              ) : (
                "Start Safe Recovery"
              )}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
