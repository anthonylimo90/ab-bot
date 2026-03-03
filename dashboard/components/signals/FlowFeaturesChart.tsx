"use client";

import { useState } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { useFlowFeaturesQuery } from "@/hooks/queries/useSignalsQuery";
import { Search } from "lucide-react";
import {
  ResponsiveContainer,
  ComposedChart,
  Bar,
  Line,
  Area,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
} from "recharts";

function formatTime(dateStr: string) {
  const d = new Date(dateStr);
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export function FlowFeaturesChart() {
  const [conditionId, setConditionId] = useState("");
  const [inputValue, setInputValue] = useState("");
  const { data: features = [], isLoading } = useFlowFeaturesQuery(
    conditionId,
    60,
  );

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    setConditionId(inputValue.trim());
  };

  // Reverse so time flows left-to-right (oldest → newest)
  const chartData = [...features].reverse().map((f) => ({
    time: formatTime(f.window_end),
    buy_volume: f.buy_volume,
    sell_volume: -Math.abs(f.sell_volume),
    imbalance_ratio: f.imbalance_ratio,
    smart_money_flow: f.smart_money_flow,
  }));

  return (
    <Card className="flex flex-col">
      <CardHeader>
        <CardTitle>Flow Features</CardTitle>
        <form onSubmit={handleSubmit} className="mt-2">
          <div className="relative">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              type="text"
              value={inputValue}
              onChange={(e) => setInputValue(e.target.value)}
              placeholder="Enter condition ID..."
              className="pl-10 text-sm"
            />
          </div>
        </form>
      </CardHeader>
      <CardContent className="flex-1">
        {!conditionId ? (
          <div className="flex flex-col items-center justify-center h-[300px] text-center">
            <Search className="h-10 w-10 text-muted-foreground/50 mb-3" />
            <p className="text-sm text-muted-foreground">
              Enter a condition ID to view flow features
            </p>
            <p className="text-xs text-muted-foreground mt-1">
              Volume, imbalance, and smart money flow data
            </p>
          </div>
        ) : isLoading ? (
          <Skeleton className="h-[300px] w-full" />
        ) : features.length === 0 ? (
          <div className="flex items-center justify-center h-[300px] text-sm text-muted-foreground">
            No flow data for this condition
          </div>
        ) : (
          <ResponsiveContainer width="100%" height={300}>
            <ComposedChart
              data={chartData}
              margin={{ top: 5, right: 5, bottom: 5, left: 5 }}
            >
              <CartesianGrid
                strokeDasharray="3 3"
                stroke="hsl(var(--border))"
              />
              <XAxis
                dataKey="time"
                tick={{ fontSize: 11 }}
                stroke="hsl(var(--muted-foreground))"
              />
              <YAxis
                yAxisId="volume"
                tick={{ fontSize: 11 }}
                stroke="hsl(var(--muted-foreground))"
              />
              <YAxis
                yAxisId="ratio"
                orientation="right"
                tick={{ fontSize: 11 }}
                stroke="hsl(var(--muted-foreground))"
                domain={[-1, 1]}
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
              <Legend wrapperStyle={{ fontSize: "12px" }} />
              <Bar
                yAxisId="volume"
                dataKey="buy_volume"
                name="Buy Volume"
                fill="#22c55e"
                opacity={0.7}
                barSize={12}
              />
              <Bar
                yAxisId="volume"
                dataKey="sell_volume"
                name="Sell Volume"
                fill="#ef4444"
                opacity={0.7}
                barSize={12}
              />
              <Area
                yAxisId="volume"
                dataKey="smart_money_flow"
                name="Smart Money"
                fill="#8b5cf6"
                fillOpacity={0.15}
                stroke="#8b5cf6"
                strokeWidth={1}
              />
              <Line
                yAxisId="ratio"
                dataKey="imbalance_ratio"
                name="Imbalance"
                stroke="#3b82f6"
                strokeWidth={2}
                strokeDasharray="5 5"
                dot={false}
              />
            </ComposedChart>
          </ResponsiveContainer>
        )}
      </CardContent>
    </Card>
  );
}
