"use client";

import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
  ReferenceLine,
  Cell,
} from "recharts";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Tooltip as UITooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useCalibrationReportQuery } from "@/hooks/queries/useDiscoverQuery";
import { Info } from "lucide-react";

interface BucketData {
  range: string;
  predicted: number;
  observed: number;
  count: number;
  gap: number;
}

const CustomTooltip = ({ active, payload }: any) => {
  if (active && payload && payload.length > 0) {
    const data = payload[0].payload as BucketData;
    return (
      <div className="bg-popover/90 backdrop-blur border rounded-lg px-3 py-2 text-sm shadow-lg space-y-1">
        <div className="font-medium">{data.range}</div>
        <div className="flex justify-between gap-4">
          <span className="text-muted-foreground">Predicted:</span>
          <span className="tabular-nums">{(data.predicted * 100).toFixed(1)}%</span>
        </div>
        <div className="flex justify-between gap-4">
          <span className="text-muted-foreground">Observed:</span>
          <span className="tabular-nums">{(data.observed * 100).toFixed(1)}%</span>
        </div>
        <div className="flex justify-between gap-4">
          <span className="text-muted-foreground">Gap:</span>
          <span className="tabular-nums">{(data.gap * 100).toFixed(1)}pp</span>
        </div>
        <div className="text-xs text-muted-foreground">
          {data.count} prediction{data.count !== 1 ? "s" : ""}
        </div>
      </div>
    );
  }
  return null;
};

function eceQuality(ece: number): { label: string; color: string } {
  if (ece <= 0.05) return { label: "Excellent", color: "text-green-500" };
  if (ece <= 0.10) return { label: "Good", color: "text-emerald-500" };
  if (ece <= 0.15) return { label: "Fair", color: "text-yellow-500" };
  return { label: "Poor", color: "text-red-500" };
}

export function CalibrationChart() {
  const { data: report, isLoading, isError } = useCalibrationReportQuery();

  if (isLoading) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Prediction Calibration</CardTitle>
        </CardHeader>
        <CardContent>
          <Skeleton className="h-48 w-full" />
        </CardContent>
      </Card>
    );
  }

  if (isError || !report) {
    return null;
  }

  if (report.total_predictions === 0) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="text-sm flex items-center gap-2">
            Prediction Calibration
            <TooltipProvider>
              <UITooltip>
                <TooltipTrigger>
                  <Info className="h-3.5 w-3.5 text-muted-foreground" />
                </TooltipTrigger>
                <TooltipContent className="max-w-xs">
                  <p>
                    Shows how well our predictions match actual outcomes.
                    Available after 30+ days of prediction data.
                  </p>
                </TooltipContent>
              </UITooltip>
            </TooltipProvider>
          </CardTitle>
        </CardHeader>
        <CardContent className="text-center py-6">
          <p className="text-sm text-muted-foreground">
            Not enough prediction data yet. Calibration requires 30+ days of
            tracked predictions.
          </p>
        </CardContent>
      </Card>
    );
  }

  const chartData: BucketData[] = report.buckets
    .filter((b) => b.count > 0)
    .map((b) => ({
      range: `${(b.lower * 100).toFixed(0)}-${(b.upper * 100).toFixed(0)}%`,
      predicted: b.avg_predicted,
      observed: b.observed_rate,
      count: b.count,
      gap: b.gap,
    }));

  const quality = eceQuality(report.ece);

  return (
    <Card>
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between">
          <CardTitle className="text-sm flex items-center gap-2">
            Prediction Calibration
            <TooltipProvider>
              <UITooltip>
                <TooltipTrigger>
                  <Info className="h-3.5 w-3.5 text-muted-foreground" />
                </TooltipTrigger>
                <TooltipContent className="max-w-xs">
                  <p>
                    Reliability diagram: compares predicted probabilities against
                    actual success rates. Bars near the diagonal line indicate
                    well-calibrated predictions.
                  </p>
                </TooltipContent>
              </UITooltip>
            </TooltipProvider>
          </CardTitle>
          <div className="flex items-center gap-2">
            <span className="text-xs text-muted-foreground">ECE:</span>
            <Badge variant="outline" className="text-xs">
              <span className={quality.color}>
                {(report.ece * 100).toFixed(1)}% ({quality.label})
              </span>
            </Badge>
          </div>
        </div>
      </CardHeader>
      <CardContent>
        <ResponsiveContainer width="100%" height={200}>
          <BarChart data={chartData} margin={{ top: 5, right: 5, left: -20, bottom: 5 }}>
            <CartesianGrid strokeDasharray="3 3" className="stroke-muted" />
            <XAxis
              dataKey="range"
              tick={{ fontSize: 10 }}
              className="fill-muted-foreground"
            />
            <YAxis
              tick={{ fontSize: 10 }}
              domain={[0, 1]}
              tickFormatter={(v) => `${(v * 100).toFixed(0)}%`}
              className="fill-muted-foreground"
            />
            <Tooltip content={<CustomTooltip />} />
            {/* Perfect calibration line */}
            <ReferenceLine
              y={0}
              stroke="transparent"
              label=""
            />
            <Bar dataKey="observed" name="Observed" radius={[2, 2, 0, 0]}>
              {chartData.map((entry, index) => (
                <Cell
                  key={index}
                  fill={entry.gap > 0.15 ? "hsl(0, 70%, 55%)" : entry.gap > 0.08 ? "hsl(45, 80%, 55%)" : "hsl(142, 70%, 45%)"}
                />
              ))}
            </Bar>
          </BarChart>
        </ResponsiveContainer>
        <div className="flex items-center justify-between mt-2 text-xs text-muted-foreground">
          <span>{report.total_predictions} total predictions</span>
          <span>
            Recommended threshold:{" "}
            <span className="font-medium text-foreground">
              {(report.recommended_threshold * 100).toFixed(0)}%
            </span>
          </span>
        </div>
      </CardContent>
    </Card>
  );
}
