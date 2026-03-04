"use client";

import { useState } from "react";
import { StrategyPerformanceTable } from "@/components/signals/StrategyPerformanceTable";
import { RecentSignalsFeed } from "@/components/signals/RecentSignalsFeed";
import { FlowFeaturesChart } from "@/components/signals/FlowFeaturesChart";
import { SignalFunnel } from "@/components/signals/SignalFunnel";
import { SkipReasonChart } from "@/components/signals/SkipReasonChart";
import { MarketRegimeBadge } from "@/components/shared/MarketRegimeBadge";
import { Zap } from "lucide-react";

export default function SignalsPage() {
  const [activeConditionId, setActiveConditionId] = useState<string | undefined>();

  return (
    <div className="space-y-5 sm:space-y-6 p-6">
      {/* Header */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h1 className="flex items-center gap-2 text-2xl font-bold tracking-tight sm:text-3xl">
            <Zap className="h-8 w-8" />
            Quant Signals
          </h1>
          <p className="text-muted-foreground">
            Multi-strategy quantitative signal dashboard
          </p>
        </div>
        <MarketRegimeBadge />
      </div>

      {/* Strategy Performance — full width */}
      <StrategyPerformanceTable />

      {/* Signal Funnel — full width */}
      <SignalFunnel />

      {/* Two-column: Recent Signals + Flow Features / Skip Reasons */}
      <div className="grid gap-4 sm:gap-6 lg:grid-cols-5">
        <div className="lg:col-span-3">
          <RecentSignalsFeed onConditionClick={setActiveConditionId} />
        </div>
        <div className="lg:col-span-2 space-y-4 sm:space-y-6">
          <FlowFeaturesChart initialConditionId={activeConditionId} />
          <SkipReasonChart />
        </div>
      </div>
    </div>
  );
}
