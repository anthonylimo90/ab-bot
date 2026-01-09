'use client';

import { Skeleton as BaseSkeleton } from '@/components/ui/skeleton';
import { Card, CardContent, CardHeader } from '@/components/ui/card';

// Re-export the base skeleton for convenience
export const Skeleton = BaseSkeleton;

export function MetricCardSkeleton() {
  return (
    <Card>
      <CardContent className="p-6">
        <div className="flex flex-col gap-2">
          <BaseSkeleton className="h-4 w-24" />
          <BaseSkeleton className="h-8 w-32" />
          <BaseSkeleton className="h-3 w-20" />
        </div>
      </CardContent>
    </Card>
  );
}

export function ChartSkeleton({ height = 300 }: { height?: number }) {
  return (
    <div
      className="relative rounded-lg bg-muted/30 overflow-hidden"
      style={{ height }}
    >
      <div className="absolute inset-0 flex items-end justify-around px-4 pb-4">
        {Array.from({ length: 20 }).map((_, i) => (
          <BaseSkeleton
            key={i}
            className="w-2 rounded-t"
            style={{
              height: `${20 + Math.random() * 60}%`,
              animationDelay: `${i * 50}ms`,
            }}
          />
        ))}
      </div>
      <div className="absolute top-4 left-4">
        <BaseSkeleton className="h-4 w-24 mb-2" />
        <BaseSkeleton className="h-6 w-32" />
      </div>
    </div>
  );
}

export function TableRowSkeleton({ columns = 6 }: { columns?: number }) {
  return (
    <tr className="border-b">
      {Array.from({ length: columns }).map((_, i) => (
        <td key={i} className="p-4">
          <BaseSkeleton className="h-4 w-full max-w-[100px]" />
        </td>
      ))}
    </tr>
  );
}

export function PositionTableSkeleton({ rows = 5 }: { rows?: number }) {
  return (
    <Card>
      <CardContent className="p-0">
        <div className="overflow-x-auto">
          <table className="w-full">
            <thead className="border-b bg-muted/50">
              <tr>
                {['Market', 'Side', 'Qty', 'Entry', 'Current', 'P&L', 'Actions'].map(
                  (header) => (
                    <th key={header} className="text-left p-4 font-medium">
                      {header}
                    </th>
                  )
                )}
              </tr>
            </thead>
            <tbody>
              {Array.from({ length: rows }).map((_, i) => (
                <TableRowSkeleton key={i} columns={7} />
              ))}
            </tbody>
          </table>
        </div>
      </CardContent>
    </Card>
  );
}

export function ActivityItemSkeleton() {
  return (
    <div className="flex items-start gap-3">
      <BaseSkeleton className="h-4 w-4 rounded mt-1" />
      <div className="flex-1 space-y-2">
        <BaseSkeleton className="h-4 w-3/4" />
        <BaseSkeleton className="h-3 w-1/2" />
        <BaseSkeleton className="h-3 w-16" />
      </div>
      <BaseSkeleton className="h-4 w-16" />
    </div>
  );
}

export function ActivityFeedSkeleton({ items = 4 }: { items?: number }) {
  return (
    <div className="space-y-4">
      {Array.from({ length: items }).map((_, i) => (
        <ActivityItemSkeleton key={i} />
      ))}
    </div>
  );
}

export function WalletCardSkeleton() {
  return (
    <Card>
      <CardContent className="p-6">
        <div className="flex flex-col gap-6">
          <div className="flex items-center gap-4">
            <BaseSkeleton className="h-10 w-10 rounded-full" />
            <div className="space-y-2">
              <BaseSkeleton className="h-4 w-32" />
              <BaseSkeleton className="h-3 w-24" />
            </div>
          </div>
          <div className="grid grid-cols-5 gap-4">
            {Array.from({ length: 5 }).map((_, i) => (
              <div key={i} className="space-y-1">
                <BaseSkeleton className="h-3 w-12" />
                <BaseSkeleton className="h-4 w-16" />
              </div>
            ))}
          </div>
          <BaseSkeleton className="h-20 w-full rounded-lg" />
        </div>
      </CardContent>
    </Card>
  );
}

export function AllocationPieSkeleton() {
  return (
    <div className="space-y-4">
      <div className="flex justify-center">
        <BaseSkeleton className="h-[200px] w-[200px] rounded-full" />
      </div>
      <div className="space-y-3">
        {Array.from({ length: 4 }).map((_, i) => (
          <div key={i} className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <BaseSkeleton className="h-3 w-3 rounded-full" />
              <BaseSkeleton className="h-4 w-20" />
            </div>
            <BaseSkeleton className="h-4 w-12" />
          </div>
        ))}
      </div>
    </div>
  );
}

export function DashboardSkeleton() {
  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="space-y-2">
          <BaseSkeleton className="h-8 w-32" />
          <BaseSkeleton className="h-4 w-48" />
        </div>
        <BaseSkeleton className="h-10 w-36" />
      </div>

      {/* Stats Grid */}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <MetricCardSkeleton key={i} />
        ))}
      </div>

      {/* Charts */}
      <div className="grid gap-6 lg:grid-cols-3">
        <Card className="lg:col-span-2">
          <CardHeader>
            <BaseSkeleton className="h-6 w-32" />
          </CardHeader>
          <CardContent>
            <ChartSkeleton />
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <BaseSkeleton className="h-6 w-40" />
          </CardHeader>
          <CardContent>
            <AllocationPieSkeleton />
          </CardContent>
        </Card>
      </div>

      {/* Activity & Recommendations */}
      <div className="grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <BaseSkeleton className="h-6 w-32" />
          </CardHeader>
          <CardContent>
            <ActivityFeedSkeleton />
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <BaseSkeleton className="h-6 w-40" />
          </CardHeader>
          <CardContent>
            <div className="space-y-4">
              {Array.from({ length: 2 }).map((_, i) => (
                <BaseSkeleton key={i} className="h-36 w-full rounded-lg" />
              ))}
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
