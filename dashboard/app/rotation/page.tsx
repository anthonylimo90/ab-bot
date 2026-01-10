'use client';

import { Card, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { useRosterStore } from '@/stores/roster-store';
import { useToastStore } from '@/stores/toast-store';
import {
  useRotationRecommendationsQuery,
  useDismissRecommendation,
  useAcceptRecommendation,
  type RotationRecommendation,
  type RecommendationType,
  type RecommendationReason,
  type Urgency,
} from '@/hooks/queries';
import { shortenAddress } from '@/lib/utils';
import {
  RefreshCw,
  AlertTriangle,
  TrendingDown,
  ArrowUpRight,
  ArrowDownRight,
  CheckCircle,
  XCircle,
  Clock,
  AlertCircle,
} from 'lucide-react';

const reasonLabels: Record<RecommendationReason, string> = {
  alpha_decay: 'Alpha Decay',
  martingale_pattern: 'Martingale Pattern',
  strategy_drift: 'Strategy Drift',
  honeypot_warning: 'Honeypot Warning',
  outperforming: 'Outperforming',
  high_risk: 'High Risk',
  consistent_losses: 'Consistent Losses',
};

const reasonIcons: Record<RecommendationReason, React.ReactNode> = {
  alpha_decay: <TrendingDown className="h-5 w-5 text-loss" />,
  martingale_pattern: <AlertTriangle className="h-5 w-5 text-yellow-500" />,
  strategy_drift: <RefreshCw className="h-5 w-5 text-blue-500" />,
  honeypot_warning: <AlertTriangle className="h-5 w-5 text-loss" />,
  outperforming: <ArrowUpRight className="h-5 w-5 text-profit" />,
  high_risk: <AlertTriangle className="h-5 w-5 text-loss" />,
  consistent_losses: <TrendingDown className="h-5 w-5 text-loss" />,
};

const urgencyColors: Record<Urgency, string> = {
  low: 'bg-blue-500/10 text-blue-500',
  medium: 'bg-yellow-500/10 text-yellow-500',
  high: 'bg-loss/10 text-loss',
};

const typeIcons: Record<RecommendationType, React.ReactNode> = {
  demote: <ArrowDownRight className="h-5 w-5 text-loss" />,
  promote: <ArrowUpRight className="h-5 w-5 text-profit" />,
  alert: <AlertTriangle className="h-5 w-5 text-yellow-500" />,
};

function RecommendationCard({
  recommendation,
  onAccept,
  onDismiss,
  isLoading,
}: {
  recommendation: RotationRecommendation;
  onAccept: () => void;
  onDismiss: () => void;
  isLoading?: boolean;
}) {
  return (
    <Card className="hover:border-primary transition-colors">
      <CardContent className="p-6">
        <div className="flex flex-col gap-4">
          {/* Header */}
          <div className="flex items-start justify-between">
            <div className="flex items-center gap-3">
              {typeIcons[recommendation.type]}
              <div>
                <p className="font-medium">
                  {recommendation.wallet_label || shortenAddress(recommendation.wallet_address)}
                </p>
                <p className="text-xs text-muted-foreground font-mono">
                  {shortenAddress(recommendation.wallet_address)}
                </p>
              </div>
            </div>
            <div className="flex items-center gap-2">
              <span className={`text-xs px-2 py-1 rounded-full ${urgencyColors[recommendation.urgency]}`}>
                {recommendation.urgency.toUpperCase()}
              </span>
            </div>
          </div>

          {/* Reason */}
          <div className="flex items-center gap-2 text-sm">
            {reasonIcons[recommendation.reason]}
            <span className="font-medium">{reasonLabels[recommendation.reason]}</span>
          </div>

          {/* Evidence */}
          <div className="space-y-2">
            <p className="text-xs text-muted-foreground font-medium uppercase">Evidence</p>
            <ul className="space-y-1">
              {recommendation.evidence.map((item, i) => (
                <li key={i} className="text-sm text-muted-foreground flex items-start gap-2">
                  <span className="text-primary">•</span>
                  {item}
                </li>
              ))}
            </ul>
          </div>

          {/* Suggested Action */}
          <div className="rounded-lg bg-muted/30 p-3">
            <p className="text-sm font-medium">{recommendation.suggested_action}</p>
          </div>

          {/* Actions */}
          <div className="flex items-center justify-between pt-2">
            <div className="flex items-center gap-1 text-xs text-muted-foreground">
              <Clock className="h-3 w-3" />
              {new Date(recommendation.created_at).toLocaleString()}
            </div>
            <div className="flex gap-2">
              <Button variant="outline" size="sm" onClick={onDismiss} disabled={isLoading}>
                <XCircle className="mr-1 h-4 w-4" />
                Dismiss
              </Button>
              <Button size="sm" onClick={onAccept} disabled={isLoading}>
                <CheckCircle className="mr-1 h-4 w-4" />
                Accept
              </Button>
            </div>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

export default function RotationPage() {
  const toast = useToastStore();
  const { demoteToBench, promoteToActive, isRosterFull } = useRosterStore();

  // Fetch recommendations from API
  const {
    data: recommendations = [],
    isLoading,
    error,
    refetch,
  } = useRotationRecommendationsQuery({ limit: 20 });

  const dismissMutation = useDismissRecommendation();
  const acceptMutation = useAcceptRecommendation();

  const handleAccept = async (rec: RotationRecommendation) => {
    if (rec.type === 'demote') {
      demoteToBench(rec.wallet_address);
      toast.success('Rotation executed', `${rec.wallet_label || shortenAddress(rec.wallet_address)} demoted to Bench`);
    } else if (rec.type === 'promote') {
      if (isRosterFull()) {
        toast.error('Roster Full', 'Demote a wallet first to make room');
        return;
      }
      promoteToActive(rec.wallet_address);
      toast.success('Rotation executed', `${shortenAddress(rec.wallet_address)} promoted to Active 5`);
    } else {
      toast.info('Alert acknowledged', 'Wallet will continue to be monitored');
    }

    await acceptMutation.mutateAsync(rec.id);
  };

  const handleDismiss = async (id: string) => {
    await dismissMutation.mutateAsync(id);
    toast.info('Recommendation dismissed');
  };

  const highUrgency = recommendations.filter((r) => r.urgency === 'high');
  const otherRecommendations = recommendations.filter((r) => r.urgency !== 'high');
  const isMutating = dismissMutation.isPending || acceptMutation.isPending;

  return (
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight flex items-center gap-2">
            <RefreshCw className="h-8 w-8" />
            Rotation
          </h1>
          <p className="text-muted-foreground">
            Automated recommendations for roster changes
          </p>
        </div>
        <Button variant="outline" onClick={() => refetch()} disabled={isLoading}>
          <RefreshCw className={`mr-2 h-4 w-4 ${isLoading ? 'animate-spin' : ''}`} />
          Refresh
        </Button>
      </div>

      {/* Error State */}
      {error && (
        <Card className="border-destructive">
          <CardContent className="p-6 text-center">
            <AlertCircle className="h-8 w-8 mx-auto mb-2 text-destructive" />
            <p className="text-destructive font-medium">Failed to load recommendations</p>
            <p className="text-sm text-muted-foreground mt-1">
              {error instanceof Error ? error.message : 'Please try again'}
            </p>
            <Button variant="outline" size="sm" className="mt-4" onClick={() => refetch()}>
              Retry
            </Button>
          </CardContent>
        </Card>
      )}

      {/* Loading State */}
      {isLoading && !error && (
        <div className="space-y-4">
          <div className="grid gap-4 md:grid-cols-3">
            {[1, 2, 3].map((i) => (
              <Card key={i}>
                <CardContent className="p-4 flex items-center gap-3">
                  <Skeleton className="h-10 w-10 rounded-full" />
                  <div className="space-y-2">
                    <Skeleton className="h-4 w-20" />
                    <Skeleton className="h-6 w-12" />
                  </div>
                </CardContent>
              </Card>
            ))}
          </div>
          {[1, 2].map((i) => (
            <Card key={i}>
              <CardContent className="p-6">
                <div className="space-y-4">
                  <div className="flex items-center gap-3">
                    <Skeleton className="h-5 w-5" />
                    <Skeleton className="h-5 w-32" />
                  </div>
                  <Skeleton className="h-4 w-full" />
                  <Skeleton className="h-4 w-3/4" />
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      )}

      {/* Summary Stats */}
      {!isLoading && !error && (
        <div className="grid gap-4 md:grid-cols-3">
          <Card>
            <CardContent className="p-4 flex items-center gap-3">
              <div className="h-10 w-10 rounded-full bg-loss/10 flex items-center justify-center">
                <AlertTriangle className="h-5 w-5 text-loss" />
              </div>
              <div>
                <p className="text-sm text-muted-foreground">High Urgency</p>
                <p className="text-2xl font-bold">{highUrgency.length}</p>
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardContent className="p-4 flex items-center gap-3">
              <div className="h-10 w-10 rounded-full bg-primary/10 flex items-center justify-center">
                <RefreshCw className="h-5 w-5 text-primary" />
              </div>
              <div>
                <p className="text-sm text-muted-foreground">Total Recommendations</p>
                <p className="text-2xl font-bold">{recommendations.length}</p>
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardContent className="p-4 flex items-center gap-3">
              <div className="h-10 w-10 rounded-full bg-profit/10 flex items-center justify-center">
                <CheckCircle className="h-5 w-5 text-profit" />
              </div>
              <div>
                <p className="text-sm text-muted-foreground">Actions This Week</p>
                <p className="text-2xl font-bold">3</p>
              </div>
            </CardContent>
          </Card>
        </div>
      )}

      {/* High Urgency Section */}
      {!isLoading && !error && highUrgency.length > 0 && (
        <div className="space-y-4">
          <div className="flex items-center gap-2">
            <AlertTriangle className="h-5 w-5 text-loss" />
            <h2 className="text-xl font-semibold">Requires Immediate Attention</h2>
          </div>
          <div className="grid gap-4">
            {highUrgency.map((rec) => (
              <RecommendationCard
                key={rec.id}
                recommendation={rec}
                onAccept={() => handleAccept(rec)}
                onDismiss={() => handleDismiss(rec.id)}
                isLoading={isMutating}
              />
            ))}
          </div>
        </div>
      )}

      {/* Other Recommendations */}
      {!isLoading && !error && otherRecommendations.length > 0 && (
        <div className="space-y-4">
          <h2 className="text-xl font-semibold">Other Recommendations</h2>
          <div className="grid gap-4">
            {otherRecommendations.map((rec) => (
              <RecommendationCard
                key={rec.id}
                recommendation={rec}
                onAccept={() => handleAccept(rec)}
                onDismiss={() => handleDismiss(rec.id)}
                isLoading={isMutating}
              />
            ))}
          </div>
        </div>
      )}

      {/* Empty State */}
      {!isLoading && !error && recommendations.length === 0 && (
        <Card>
          <CardContent className="p-12 text-center">
            <CheckCircle className="h-12 w-12 mx-auto mb-4 text-profit" />
            <h3 className="text-lg font-medium mb-2">All Clear!</h3>
            <p className="text-muted-foreground">
              No rotation recommendations at this time. Your roster is performing well.
            </p>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
