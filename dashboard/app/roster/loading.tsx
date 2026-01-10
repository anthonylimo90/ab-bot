'use client';

import { Card, CardContent, CardHeader } from '@/components/ui/card';
import { WalletCardSkeleton, Skeleton } from '@/components/shared/Skeletons';

export default function RosterLoading() {
  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="space-y-2">
          <Skeleton className="h-8 w-40" />
          <Skeleton className="h-4 w-64" />
        </div>
        <div className="flex gap-2">
          <Skeleton className="h-10 w-32" />
        </div>
      </div>

      {/* Stats Overview */}
      <Card>
        <CardContent className="p-6">
          <div className="grid grid-cols-4 gap-6">
            {Array.from({ length: 4 }).map((_, i) => (
              <div key={i} className="space-y-1">
                <Skeleton className="h-4 w-24" />
                <Skeleton className="h-6 w-32" />
              </div>
            ))}
          </div>
        </CardContent>
      </Card>

      {/* Wallet Cards */}
      <div className="space-y-4">
        {Array.from({ length: 5 }).map((_, i) => (
          <WalletCardSkeleton key={i} />
        ))}
      </div>
    </div>
  );
}
