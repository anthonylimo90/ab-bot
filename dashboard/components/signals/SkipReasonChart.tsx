"use client";

import { useMemo } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { InfoTooltip } from "@/components/shared/InfoTooltip";
import { useRecentSignalsQuery } from "@/hooks/queries/useSignalsQuery";
import {
  ResponsiveContainer,
  BarChart,
  Bar,
  XAxis,
  YAxis,
  Tooltip,
  CartesianGrid,
} from "recharts";

export function SkipReasonChart() {
  const { data: signals = [] } = useRecentSignalsQuery({ limit: 200 });

  const chartData = useMemo(() => {
    const counts = new Map<string, number>();
    for (const s of signals) {
      if (s.execution_status === "skipped" && s.skip_reason) {
        counts.set(s.skip_reason, (counts.get(s.skip_reason) ?? 0) + 1);
      }
    }
    return Array.from(counts.entries())
      .map(([reason, count]) => ({ reason, count }))
      .sort((a, b) => b.count - a.count);
  }, [signals]);

  if (chartData.length === 0) {
    return (
      <Card>
        <CardHeader className="pb-3">
          <CardTitle className="flex items-center gap-2 text-base">
            <span>Skip Reasons</span>
            <InfoTooltip content="This summarizes the most common reasons the system decided not to place a trade after generating a signal." />
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="flex items-center justify-center h-[200px] text-sm text-muted-foreground">
            No skipped signals
          </div>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center gap-2 text-base">
          <span>Skip Reasons</span>
          <InfoTooltip content="This summarizes the most common reasons the system decided not to place a trade after generating a signal." />
        </CardTitle>
      </CardHeader>
      <CardContent>
        <ResponsiveContainer width="100%" height={Math.max(200, chartData.length * 36)}>
          <BarChart
            data={chartData}
            layout="vertical"
            margin={{ top: 5, right: 20, bottom: 5, left: 5 }}
          >
            <CartesianGrid
              strokeDasharray="3 3"
              stroke="hsl(var(--border))"
              horizontal={false}
            />
            <XAxis
              type="number"
              tick={{ fontSize: 11 }}
              stroke="hsl(var(--muted-foreground))"
              allowDecimals={false}
            />
            <YAxis
              type="category"
              dataKey="reason"
              tick={{ fontSize: 11 }}
              stroke="hsl(var(--muted-foreground))"
              width={120}
            />
            <Tooltip
              contentStyle={{
                backgroundColor: "hsl(var(--popover))",
                borderColor: "hsl(var(--border))",
                borderRadius: "8px",
                fontSize: "12px",
              }}
              labelStyle={{ color: "hsl(var(--muted-foreground))" }}
            />
            <Bar
              dataKey="count"
              name="Count"
              fill="hsl(var(--muted-foreground))"
              opacity={0.7}
              radius={[0, 4, 4, 0]}
              barSize={20}
            />
          </BarChart>
        </ResponsiveContainer>
      </CardContent>
    </Card>
  );
}
