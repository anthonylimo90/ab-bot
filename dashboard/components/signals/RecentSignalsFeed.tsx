"use client";

import { useState } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Skeleton } from "@/components/ui/skeleton";
import { useRecentSignalsQuery } from "@/hooks/queries/useSignalsQuery";
import { formatTimeAgo, cn } from "@/lib/utils";
import { ArrowUp, ArrowDown } from "lucide-react";

type KindFilter = "all" | "flow" | "cross_market" | "mean_reversion" | "resolution_proximity";

const KIND_LABELS: Record<string, string> = {
  flow: "Flow",
  cross_market: "Cross Mkt",
  mean_reversion: "Mean Rev",
  resolution_proximity: "Resolution",
};

const KIND_BADGE_STYLES: Record<string, string> = {
  flow: "bg-blue-500/10 text-blue-600 border-blue-500/20",
  cross_market: "bg-purple-500/10 text-purple-600 border-purple-500/20",
  mean_reversion: "bg-amber-500/10 text-amber-600 border-amber-500/20",
  resolution_proximity: "bg-green-500/10 text-green-600 border-green-500/20",
};

const STATUS_STYLES: Record<string, string> = {
  pending: "bg-yellow-500/10 text-yellow-600 border-yellow-500/20",
  executed: "bg-green-500/10 text-green-600 border-green-500/20",
  skipped: "bg-muted text-muted-foreground border-muted-foreground/20",
  expired: "bg-red-500/10 text-red-600 border-red-500/20",
};

export function RecentSignalsFeed() {
  const [kindFilter, setKindFilter] = useState<KindFilter>("all");
  const queryKind = kindFilter === "all" ? undefined : kindFilter;
  const { data: signals = [], isLoading } = useRecentSignalsQuery({
    kind: queryKind,
    limit: 50,
  });

  return (
    <Card className="flex flex-col">
      <CardHeader>
        <div className="flex items-center justify-between">
          <CardTitle>Recent Signals</CardTitle>
          <Badge variant="secondary">{signals.length}</Badge>
        </div>
        <Tabs
          value={kindFilter}
          onValueChange={(v) => setKindFilter(v as KindFilter)}
        >
          <div className="overflow-x-auto">
            <TabsList className="w-max">
              <TabsTrigger value="all">All</TabsTrigger>
              <TabsTrigger value="flow">Flow</TabsTrigger>
              <TabsTrigger value="cross_market">Cross Mkt</TabsTrigger>
              <TabsTrigger value="mean_reversion">Mean Rev</TabsTrigger>
              <TabsTrigger value="resolution_proximity">Resolution</TabsTrigger>
            </TabsList>
          </div>
        </Tabs>
      </CardHeader>
      <CardContent className="flex-1 overflow-hidden">
        {isLoading ? (
          <div className="space-y-3">
            {Array.from({ length: 5 }).map((_, i) => (
              <Skeleton key={i} className="h-14 w-full" />
            ))}
          </div>
        ) : signals.length === 0 ? (
          <p className="text-sm text-muted-foreground text-center py-8">
            No signals yet. Quantitative signals will appear here as they are generated.
          </p>
        ) : (
          <div className="max-h-[400px] overflow-y-auto space-y-2">
            {signals.map((signal) => {
              const isBuyYes = signal.direction === "BuyYes";
              const kindStyle =
                KIND_BADGE_STYLES[signal.kind] ||
                "bg-muted text-muted-foreground";
              const statusStyle =
                STATUS_STYLES[signal.execution_status || "pending"] ||
                STATUS_STYLES.pending;

              return (
                <div
                  key={signal.id}
                  className="flex items-center gap-3 p-3 rounded-lg border hover:bg-muted/30 transition-colors"
                >
                  {/* Direction arrow */}
                  <div
                    className={cn(
                      "flex h-8 w-8 shrink-0 items-center justify-center rounded-full",
                      isBuyYes
                        ? "bg-green-500/10 text-green-600"
                        : "bg-red-500/10 text-red-600",
                    )}
                  >
                    {isBuyYes ? (
                      <ArrowUp className="h-4 w-4" />
                    ) : (
                      <ArrowDown className="h-4 w-4" />
                    )}
                  </div>

                  {/* Signal info */}
                  <div className="min-w-0 flex-1 space-y-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <Badge
                        variant="outline"
                        className={cn("text-xs", kindStyle)}
                      >
                        {KIND_LABELS[signal.kind] || signal.kind}
                      </Badge>
                      <span
                        className={cn(
                          "text-xs font-medium",
                          isBuyYes ? "text-green-600" : "text-red-600",
                        )}
                      >
                        {signal.direction}
                      </span>
                      {signal.execution_status && (
                        <Badge
                          variant="outline"
                          className={cn("text-xs", statusStyle)}
                        >
                          {signal.execution_status}
                        </Badge>
                      )}
                    </div>
                    <div className="flex items-center gap-2 text-xs text-muted-foreground">
                      <span className="truncate max-w-[140px]" title={signal.condition_id}>
                        {signal.condition_id.slice(0, 8)}...
                      </span>
                      <span>{formatTimeAgo(signal.generated_at)}</span>
                      {signal.skip_reason && (
                        <span className="text-amber-600" title={signal.skip_reason}>
                          skip: {signal.skip_reason}
                        </span>
                      )}
                    </div>
                  </div>

                  {/* Confidence + size */}
                  <div className="shrink-0 text-right space-y-1">
                    <div className="flex items-center gap-1.5">
                      <div className="w-16 h-1.5 rounded-full bg-muted overflow-hidden">
                        <div
                          className={cn(
                            "h-full rounded-full",
                            signal.confidence >= 0.7
                              ? "bg-green-500"
                              : signal.confidence >= 0.5
                                ? "bg-yellow-500"
                                : "bg-red-500",
                          )}
                          style={{ width: `${signal.confidence * 100}%` }}
                        />
                      </div>
                      <span className="text-xs tabular-nums font-medium w-8">
                        {(signal.confidence * 100).toFixed(0)}%
                      </span>
                    </div>
                    {signal.size_usd != null && (
                      <span className="text-xs tabular-nums text-muted-foreground">
                        ${signal.size_usd.toFixed(2)}
                      </span>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
