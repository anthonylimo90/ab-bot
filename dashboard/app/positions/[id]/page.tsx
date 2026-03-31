"use client";

import { useParams, useRouter } from "next/navigation";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { MetricCard } from "@/components/shared/MetricCard";
import { ErrorBoundary } from "@/components/shared/ErrorBoundary";
import {
  usePositionQuery,
  useClosePositionMutation,
} from "@/hooks/queries/usePositionsQuery";
import { cn, formatCurrency } from "@/lib/utils";
import { ArrowLeft, Layers, ExternalLink } from "lucide-react";
import type { PositionState } from "@/types/api";

const STATE_BADGE_STYLES: Record<
  PositionState,
  { label: string; className: string }
> = {
  pending: {
    label: "Pending",
    className: "bg-yellow-500/10 text-yellow-600 border-yellow-500/20",
  },
  open: {
    label: "Open",
    className: "bg-green-500/10 text-green-600 border-green-500/20",
  },
  exit_ready: {
    label: "Exit Ready",
    className: "bg-blue-500/10 text-blue-600 border-blue-500/20",
  },
  closing: {
    label: "Closing",
    className: "bg-orange-500/10 text-orange-600 border-orange-500/20",
  },
  closed: {
    label: "Closed",
    className: "bg-muted text-muted-foreground border-muted-foreground/20",
  },
  entry_failed: {
    label: "Entry Failed",
    className: "bg-red-500/10 text-red-600 border-red-500/20",
  },
  exit_failed: {
    label: "Exit Failed",
    className: "bg-red-500/10 text-red-600 border-red-500/20",
  },
  stalled: {
    label: "Stalled",
    className: "bg-purple-500/10 text-purple-600 border-purple-500/20",
  },
};

function StateBadge({ state }: { state?: PositionState }) {
  const s = state ?? "open";
  const style = STATE_BADGE_STYLES[s] ?? STATE_BADGE_STYLES.open;
  return (
    <Badge variant="outline" className={cn("text-xs", style.className)}>
      {style.label}
    </Badge>
  );
}

function formatDuration(openedAt: string, closedAt?: string): string {
  const start = new Date(openedAt).getTime();
  const end = closedAt ? new Date(closedAt).getTime() : Date.now();
  const diffMs = end - start;
  const hours = Math.floor(diffMs / 3600000);
  const mins = Math.floor((diffMs % 3600000) / 60000);
  if (hours >= 24) {
    const days = Math.floor(hours / 24);
    return `${days}d ${hours % 24}h`;
  }
  return hours > 0 ? `${hours}h ${mins}m` : `${mins}m`;
}

function DetailRow({
  label,
  value,
  className,
}: {
  label: string;
  value: React.ReactNode;
  className?: string;
}) {
  return (
    <div className="flex items-center justify-between py-2 border-b border-border/50 last:border-0">
      <span className="text-sm text-muted-foreground">{label}</span>
      <span className={cn("text-sm font-medium tabular-nums", className)}>
        {value}
      </span>
    </div>
  );
}

const isOpen = (state?: PositionState) =>
  state && !["closed", "entry_failed"].includes(state);

export default function PositionDetailPage() {
  const params = useParams<{ id: string }>();
  const router = useRouter();
  const { data: position, isLoading, error } = usePositionQuery(params.id);
  const closeMutation = useClosePositionMutation();

  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-24">
        <div className="h-8 w-8 animate-spin rounded-full border-2 border-primary border-t-transparent" />
      </div>
    );
  }

  if (error || !position) {
    return (
      <div className="space-y-4">
        <Button variant="ghost" size="sm" onClick={() => router.push("/positions")}>
          <ArrowLeft className="mr-1 h-4 w-4" /> Positions
        </Button>
        <Card>
          <CardContent className="py-12 text-center text-muted-foreground">
            Position not found
          </CardContent>
        </Card>
      </div>
    );
  }

  const entryValue = position.entry_price * position.quantity;
  const pnl = isOpen(position.state)
    ? position.unrealized_pnl
    : (position.realized_pnl ?? 0);
  const pnlPct =
    entryValue > 0 ? (pnl / entryValue) * 100 : 0;

  return (
    <ErrorBoundary>
      <div className="space-y-5 sm:space-y-6">
        {/* Header */}
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center gap-3">
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8"
              onClick={() => router.push("/positions")}
            >
              <ArrowLeft className="h-4 w-4" />
            </Button>
            <Layers className="h-5 w-5 text-muted-foreground" />
            <div>
              <h1 className="text-xl font-bold">Position Detail</h1>
              <p className="text-xs text-muted-foreground font-mono">
                {position.id}
              </p>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <StateBadge state={position.state} />
            {isOpen(position.state) && (
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button
                    variant="destructive"
                    size="sm"
                    disabled={closeMutation.isPending}
                  >
                    Request Exit
                  </Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>Request Exit?</AlertDialogTitle>
                    <AlertDialogDescription>
                      This will queue the canonical exit flow for your{" "}
                      {position.outcome.toUpperCase()} position (
                      {position.quantity.toFixed(2)} shares). Current P&amp;L:{" "}
                      {formatCurrency(position.unrealized_pnl, { showSign: true })}
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                    <AlertDialogAction
                      onClick={() =>
                        closeMutation.mutate({ positionId: position.id })
                      }
                      className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                    >
                      Queue Exit
                    </AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            )}
          </div>
        </div>

        {/* Metric Cards */}
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
          <MetricCard
            title={isOpen(position.state) ? "Unrealized P&L" : "Realized P&L"}
            value={formatCurrency(pnl, { showSign: true })}
            trend={pnl > 0 ? "up" : pnl < 0 ? "down" : "neutral"}
            changeLabel={`${pnlPct >= 0 ? "+" : ""}${pnlPct.toFixed(2)}%`}
          />
          <MetricCard
            title="Entry Value"
            value={formatCurrency(entryValue)}
            trend="neutral"
          />
          <MetricCard
            title="Quantity"
            value={position.quantity.toFixed(2)}
            trend="neutral"
          />
          <MetricCard
            title="Duration"
            value={formatDuration(
              position.opened_at,
              isOpen(position.state) ? undefined : position.updated_at,
            )}
            trend="neutral"
          />
        </div>

        {/* Market Info */}
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="text-base">Market</CardTitle>
          </CardHeader>
          <CardContent className="space-y-0">
            <DetailRow
              label="Market ID"
              value={
                <span className="font-mono text-xs break-all">
                  {position.market_id}
                </span>
              }
            />
            <DetailRow
              label="Side"
              value={
                <Badge
                  variant="outline"
                  className={cn(
                    "text-xs",
                    position.outcome === "yes"
                      ? "bg-green-500/10 text-green-600 border-green-500/20"
                      : "bg-red-500/10 text-red-600 border-red-500/20",
                  )}
                >
                  {position.outcome.toUpperCase()}
                </Badge>
              }
            />
            <DetailRow label="Exit Strategy" value={position.exit_strategy.replace(/_/g, " ")} />
            {position.resolution_winner && (
              <DetailRow
                label="Resolution Winner"
                value={
                  <Badge variant="outline" className="text-xs">
                    {position.resolution_winner.toUpperCase()}
                  </Badge>
                }
              />
            )}
          </CardContent>
        </Card>

        {/* Leg Breakdown */}
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="text-base">Leg Breakdown</CardTitle>
          </CardHeader>
          <CardContent className="p-0">
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b bg-muted/50">
                    <th className="px-4 py-2 text-left font-medium text-muted-foreground">
                      Leg
                    </th>
                    <th className="px-4 py-2 text-right font-medium text-muted-foreground">
                      Entry Price
                    </th>
                    <th className="px-4 py-2 text-right font-medium text-muted-foreground">
                      Exit Price
                    </th>
                    <th className="px-4 py-2 text-right font-medium text-muted-foreground">
                      Held Qty
                    </th>
                    <th className="px-4 py-2 text-right font-medium text-muted-foreground">
                      Exited Qty
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <tr className="border-b">
                    <td className="px-4 py-2 font-medium">
                      <Badge
                        variant="outline"
                        className="text-xs bg-green-500/10 text-green-600 border-green-500/20"
                      >
                        YES
                      </Badge>
                    </td>
                    <td className="px-4 py-2 text-right tabular-nums">
                      {position.entry_price > 0 && position.outcome === "yes"
                        ? position.entry_price.toFixed(4)
                        : position.held_yes_qty > 0 || position.exited_yes_qty > 0
                          ? "-"
                          : "-"}
                    </td>
                    <td className="px-4 py-2 text-right tabular-nums">
                      {position.yes_exit_price?.toFixed(4) ?? "-"}
                    </td>
                    <td className="px-4 py-2 text-right tabular-nums">
                      {position.held_yes_qty.toFixed(2)}
                    </td>
                    <td className="px-4 py-2 text-right tabular-nums">
                      {position.exited_yes_qty.toFixed(2)}
                    </td>
                  </tr>
                  <tr className="border-b">
                    <td className="px-4 py-2 font-medium">
                      <Badge
                        variant="outline"
                        className="text-xs bg-red-500/10 text-red-600 border-red-500/20"
                      >
                        NO
                      </Badge>
                    </td>
                    <td className="px-4 py-2 text-right tabular-nums">
                      {position.entry_price > 0 && position.outcome === "no"
                        ? position.entry_price.toFixed(4)
                        : position.held_no_qty > 0 || position.exited_no_qty > 0
                          ? "-"
                          : "-"}
                    </td>
                    <td className="px-4 py-2 text-right tabular-nums">
                      {position.no_exit_price?.toFixed(4) ?? "-"}
                    </td>
                    <td className="px-4 py-2 text-right tabular-nums">
                      {position.held_no_qty.toFixed(2)}
                    </td>
                    <td className="px-4 py-2 text-right tabular-nums">
                      {position.exited_no_qty.toFixed(2)}
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </CardContent>
        </Card>

        {/* Pricing */}
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="text-base">Pricing</CardTitle>
          </CardHeader>
          <CardContent className="space-y-0">
            <DetailRow
              label="Entry Price"
              value={`$${position.entry_price.toFixed(4)}`}
            />
            <DetailRow
              label={isOpen(position.state) ? "Current Price" : "Exit Price"}
              value={position.current_price != null ? `$${position.current_price.toFixed(4)}` : "-"}
            />
            <DetailRow
              label={isOpen(position.state) ? "Unrealized P&L" : "Realized P&L"}
              value={formatCurrency(pnl, { showSign: true })}
              className={cn(
                pnl > 0 ? "text-profit" : pnl < 0 ? "text-loss" : "",
              )}
            />
            <DetailRow
              label="P&L %"
              value={`${pnlPct >= 0 ? "+" : ""}${pnlPct.toFixed(2)}%`}
              className={cn(
                pnlPct > 0 ? "text-profit" : pnlPct < 0 ? "text-loss" : "",
              )}
            />
            {position.stop_loss != null && (
              <DetailRow
                label="Stop Loss"
                value={`$${position.stop_loss.toFixed(4)}`}
              />
            )}
            {position.take_profit != null && (
              <DetailRow
                label="Take Profit"
                value={`$${position.take_profit.toFixed(4)}`}
              />
            )}
          </CardContent>
        </Card>

        {/* Timeline */}
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="text-base">Timeline</CardTitle>
          </CardHeader>
          <CardContent className="space-y-0">
            <DetailRow
              label="Opened"
              value={new Date(position.opened_at).toLocaleString()}
            />
            <DetailRow
              label="Last Updated"
              value={new Date(position.updated_at).toLocaleString()}
            />
            <DetailRow
              label="Duration"
              value={formatDuration(
                position.opened_at,
                isOpen(position.state) ? undefined : position.updated_at,
              )}
            />
          </CardContent>
        </Card>
      </div>
    </ErrorBoundary>
  );
}
