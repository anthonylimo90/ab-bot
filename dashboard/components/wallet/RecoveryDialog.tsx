"use client";

import { useState } from "react";
import Link from "next/link";
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
import { cn, formatTimeAgo } from "@/lib/utils";
import { useToastStore } from "@/stores/toast-store";
import { useWalletStore, selectPrimaryWallet } from "@/stores/wallet-store";

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
  items,
  tone = "default",
}: {
  title: string;
  value: number;
  items: number;
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
        {items} item{items === 1 ? "" : "s"}
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
  const activeWallet = useWalletStore(selectPrimaryWallet);
  const previewQuery = useRecoveryPreviewQuery(workspaceId, open);
  const runMutation = useRunRecoveryMutation(workspaceId);
  const activeWalletLabel = activeWallet
    ? activeWallet.label ||
      `${activeWallet.address.slice(0, 6)}...${activeWallet.address.slice(-4)}`
    : null;

  const handleRunRecovery = async () => {
    if (!workspaceId) return;

    try {
      const result = await runMutation.mutateAsync();
      if (result.warnings.length > 0) {
        toast.warning("Recovery initiated with warnings", result.warnings.join(" "));
      } else {
        toast.success(
          "Recovery initiated",
          `Requeued ${result.safe_exit_failures_requeued} failed exits, reopened ${result.stalled_positions_reopened} stalled items, and submitted ${result.orphan_inventory_succeeded} orphan inventory exits.`,
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
        <span>Recover</span>
      </Button>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-[560px]">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <Wallet className="h-5 w-5 text-primary" />
              Safe Recovery
            </DialogTitle>
            <DialogDescription>
              Refresh reconciled inventory, re-arm canonical exit processing, and submit recoverable orphan inventory exits.
            </DialogDescription>
          </DialogHeader>

          {!workspaceId ? (
            <div className="rounded-xl border border-dashed px-4 py-6 text-sm text-muted-foreground">
              The canonical trading workspace is still loading.
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

              <div className="rounded-2xl border border-border/70 bg-card/70 px-4 py-3 text-sm">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">Inventory status</span>
                  <span
                    className={cn(
                      "rounded-full px-2 py-0.5 text-[11px] font-medium",
                      preview.inventory_backfill_in_progress
                        ? "bg-amber-100 text-amber-900"
                        : "bg-emerald-100 text-emerald-900",
                    )}
                  >
                    {preview.inventory_backfill_in_progress
                      ? "Historical backfill in progress"
                      : "Historical backfill complete"}
                  </span>
                </div>
                <div className="mt-2 text-muted-foreground">
                  {preview.inventory_last_synced_at
                    ? `Last inventory sync ${formatTimeAgo(preview.inventory_last_synced_at)}.`
                    : "Inventory sync has not completed yet."}{" "}
                  {preview.inventory_backfill_in_progress
                    ? preview.inventory_backfill_cursor_block != null
                      ? `Backfill cursor is at block ${preview.inventory_backfill_cursor_block.toLocaleString()}.`
                      : "Backfill is still warming up."
                    : preview.inventory_backfill_completed_at
                      ? `Backfill completed ${formatTimeAgo(preview.inventory_backfill_completed_at)}.`
                      : "Backfill is waiting for its first completed pass."}
                </div>
              </div>

              <div className="rounded-2xl border border-emerald-200 bg-emerald-50/70 px-4 py-3 text-sm text-emerald-950">
                <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                  <div>
                    <div className="font-medium">Where recovered funds land</div>
                    <div className="mt-1 text-emerald-900/90">
                      Recovery always settles into the active trading wallet, not a separate external address.
                    </div>
                    <div className="mt-2 text-emerald-900/90">
                      {activeWallet
                        ? `Active wallet: ${activeWalletLabel} (${activeWallet.address}). If this matches your MetaMask Polygon wallet, no withdrawal transfer is needed after recovery.`
                        : "No active wallet is selected yet. Connect a wallet and mark it active before expecting recovered funds to appear."}
                    </div>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    <Button asChild size="sm" variant="outline">
                      <Link href="/settings#wallet-account" onClick={() => setOpen(false)}>
                        View Wallet
                      </Link>
                    </Button>
                    <Button asChild size="sm" variant="outline">
                      <Link href="/settings#withdraw-usdc" onClick={() => setOpen(false)}>
                        Withdraw Elsewhere
                      </Link>
                    </Button>
                  </div>
                </div>
              </div>

              <div className="grid gap-3 sm:grid-cols-2">
                <RecoveryMetricCard
                  title="Safe Recovery Candidate"
                  value={preview.safe_recovery.marked_value}
                  items={preview.safe_recovery.positions}
                />
                <RecoveryMetricCard
                  title="Recoverable Now"
                  value={preview.recoverable_now.marked_value}
                  items={preview.recoverable_now.positions}
                />
                <RecoveryMetricCard
                  title="Liquidity Blocked"
                  value={preview.liquidity_blocked.marked_value}
                  items={preview.liquidity_blocked.positions}
                  tone="warning"
                />
                <RecoveryMetricCard
                  title="Orphan Inventory"
                  value={preview.orphan_inventory.marked_value}
                  items={preview.orphan_inventory.positions}
                  tone={preview.orphan_inventory.positions > 0 ? "warning" : "default"}
                />
              </div>

              <div className="grid gap-3 sm:grid-cols-3">
                <RecoveryMetricCard
                  title="Suspect Inventory"
                  value={preview.suspect_inventory.marked_value}
                  items={preview.suspect_inventory.positions}
                  tone="danger"
                />
                <RecoveryMetricCard
                  title="Stalled"
                  value={preview.stalled.marked_value}
                  items={preview.stalled.positions}
                  tone="warning"
                />
                <RecoveryMetricCard
                  title="Open Monitoring"
                  value={preview.open_monitoring.marked_value}
                  items={preview.open_monitoring.positions}
                />
                <RecoveryMetricCard
                  title="Other Blocked"
                  value={preview.other_blocked.marked_value}
                  items={preview.other_blocked.positions}
                />
              </div>

              {runResult && (
                <div className="rounded-2xl border border-emerald-200 bg-emerald-50/70 px-4 py-3 text-sm text-emerald-950">
                  <div className="flex items-center gap-2 font-medium">
                    <RefreshCw className="h-4 w-4" />
                    Recovery queued
                  </div>
                  <div className="mt-1 text-emerald-900/90">
                    Requeued {runResult.safe_exit_failures_requeued} safe exit failures, reopened{" "}
                    {runResult.stalled_positions_reopened} stalled items, and submitted{" "}
                    {runResult.orphan_inventory_succeeded} orphan inventory exits from{" "}
                    {runResult.orphan_inventory_attempted} attempted orphan balances.
                  </div>
                  {runResult.warnings.length > 0 && (
                    <div className="mt-2 space-y-2 text-amber-900">
                      {runResult.warnings.map((warning) => (
                        <div key={warning} className="flex items-start gap-2">
                          <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0" />
                          <span>{warning}</span>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}
            </div>
          )}

          <DialogFooter className="gap-2 sm:justify-between">
            <div className="text-xs text-muted-foreground">
              Recovery now uses reconciled inventory and canonical exit routing, and recovered funds remain in the active trading wallet unless you later send a separate withdrawal.
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
