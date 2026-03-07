"use client";

import { useState } from "react";
import { StrategyPerformanceTable } from "@/components/signals/StrategyPerformanceTable";
import { RecentSignalsFeed } from "@/components/signals/RecentSignalsFeed";
import { FlowFeaturesChart } from "@/components/signals/FlowFeaturesChart";
import { SignalFunnel } from "@/components/signals/SignalFunnel";
import { SkipReasonChart } from "@/components/signals/SkipReasonChart";
import { PageIntro } from "@/components/shared/PageIntro";
import { MarketRegimeBadge } from "@/components/shared/MarketRegimeBadge";
import { Zap } from "lucide-react";

export default function SignalsPage() {
  const [activeConditionId, setActiveConditionId] = useState<string | undefined>();

  return (
    <div className="space-y-5 sm:space-y-6">
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

      <PageIntro
        title="How to read this page"
        description="This page explains why the system is seeing possible trades and what happened after each one was evaluated."
        bullets={[
          "Start with Recent Signals to see the newest trade ideas and whether they were executed or skipped.",
          "Use Signal Funnel and Skip Reasons to understand where opportunities are being filtered out.",
          "If a condition looks interesting, click it in Recent Signals to load its flow chart on the right."
        ]}
      />

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
