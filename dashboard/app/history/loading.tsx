"use client";

import {
  MetricCardSkeleton,
  PositionTableSkeleton,
  Skeleton,
} from "@/components/shared/Skeletons";

export default function HistoryLoading() {
  return (
    <div className="space-y-6">
      <div className="space-y-2">
        <Skeleton className="h-8 w-32" />
        <Skeleton className="h-4 w-56" />
      </div>
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <MetricCardSkeleton key={i} />
        ))}
      </div>
      <div className="flex gap-2">
        <Skeleton className="h-10 w-[140px]" />
        <Skeleton className="h-10 w-[140px]" />
      </div>
      <PositionTableSkeleton rows={10} />
    </div>
  );
}
