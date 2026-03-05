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
    const skipped = signals.filter(
      (s) => s.execution_status === "skipped",
    ).length;
    const expired = signals.filter(
      (s) => s.execution_status === "expired",
    ).length;
    const executionRate =
      generated > 0 ? ((executed / generated) * 100).toFixed(1) : "0";

    return { generated, executed, skipped, expired, executionRate };
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
      color: "text-red-600",
    },
  ];

  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center gap-2 text-base">
          <span>Signal Funnel</span>
          <InfoTooltip content="Think of this as the system's decision pipeline: generated means an idea was found, executed means a trade was placed, skipped means it was rejected, and expired means the opportunity got too old." />
        </CardTitle>
      </CardHeader>
      <CardContent>
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
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
