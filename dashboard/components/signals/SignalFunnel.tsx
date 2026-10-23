"use client";

import { useMemo } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { InfoTooltip } from "@/components/shared/InfoTooltip";
import { useRecentSignalsQuery } from "@/hooks/queries/useSignalsQuery";
import { cn } from "@/lib/utils";

export function SignalFunnel() {
  const { data: signals = [] } = useRecentSignalsQuery({ limit: 200 });

  const stats = useMemo(() => {
    const generated = signals.length;
    const executed = signals.filter(
      (s) => s.execution_status === "executed",
    ).length;
    // A signal past its expiry is skipped with reason "expired"; it is
    // counted apart from the other skips.
    const expired = signals.filter(
      (s) => s.execution_status === "skipped" && s.skip_reason === "expired",
    ).length;
    const skipped =
      signals.filter((s) => s.execution_status === "skipped").length - expired;
    const failed = signals.filter(
      (s) => s.execution_status === "failed",
    ).length;
    const executionRate =
      generated > 0 ? ((executed / generated) * 100).toFixed(1) : "0";

    return { generated, executed, skipped, expired, failed, executionRate };
  }, [signals]);

  const items = [
    {
      label: "Generated",
      value: stats.generated,
      sub: null,
      color: "text-foreground",
    },
    {
      label: "Executed",
      value: stats.executed,
      sub: `${stats.executionRate}%`,
      color: "text-green-600",
    },
    {
      label: "Skipped",
      value: stats.skipped,
      sub: null,
      color: "text-muted-foreground",
    },
    {
      label: "Expired",
      value: stats.expired,
      sub: null,
      color: "text-muted-foreground",
    },
    {
      label: "Failed",
      value: stats.failed,
      sub: null,
      color: "text-red-600",
    },
  ];

  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center gap-2 text-base">
          <span>Signal Funnel</span>
          <InfoTooltip content="Think of this as the system's decision pipeline: generated means an idea was found, executed means a trade was placed, skipped means it was rejected, expired means the opportunity got too old, and failed means the order could not be placed." />
        </CardTitle>
      </CardHeader>
      <CardContent>
        <div className="grid gap-3 sm:grid-cols-3 xl:grid-cols-5">
          {items.map((item) => (
            <div
              key={item.label}
              className="rounded-lg border p-3 text-center"
            >
              <p className="text-xs text-muted-foreground mb-1">
                {item.label}
              </p>
              <p
                className={cn(
                  "text-2xl font-bold tabular-nums",
                  item.color,
                )}
              >
                {item.value}
              </p>
              {item.sub && (
                <p className="text-xs text-muted-foreground mt-0.5">
                  {item.sub} rate
                </p>
              )}
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  );
}
