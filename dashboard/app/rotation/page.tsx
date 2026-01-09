'use client';

import { useState } from 'react';
import Link from 'next/link';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { useRosterStore } from '@/stores/roster-store';
import { useToastStore } from '@/stores/toast-store';
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
} from 'lucide-react';

type RecommendationType = 'demote' | 'promote' | 'alert';
type RecommendationReason = 'alpha_decay' | 'martingale_pattern' | 'strategy_drift' | 'honeypot_warning' | 'outperforming';
type Urgency = 'low' | 'medium' | 'high';

interface RotationRecommendation {
  id: string;
  type: RecommendationType;
  walletAddress: string;
  walletLabel?: string;
  reason: RecommendationReason;
  evidence: string[];
  urgency: Urgency;
  suggestedAction: string;
  createdAt: string;
}

// Mock recommendations
const mockRecommendations: RotationRecommendation[] = [
  {
    id: '1',
    type: 'demote',
    walletAddress: '0x1234567890abcdef1234567890abcdef12345678',
    walletLabel: 'Alpha Trader',
    reason: 'alpha_decay',
    evidence: [
      '30-day ROI dropped from +47% to +12%',
      'Win rate decreased from 71% to 58%',
      'Sharpe ratio below 1.0 for 2 weeks',
    ],
    urgency: 'high',
    suggestedAction: 'Demote to Bench for monitoring',
    createdAt: '2026-01-09T10:00:00Z',
  },
  {
    id: '2',
    type: 'alert',
    walletAddress: '0xabcdef1234567890abcdef1234567890abcdef12',
    walletLabel: 'Event Specialist',
    reason: 'martingale_pattern',
    evidence: [
      'Position sizes doubled after 3 consecutive losses',
      'Risk exposure increased by 2.5x this week',
    ],
    urgency: 'medium',
    suggestedAction: 'Monitor closely for additional losses',
    createdAt: '2026-01-09T08:30:00Z',
  },
  {
    id: '3',
    type: 'promote',
    walletAddress: '0x5678901234abcdef5678901234abcdef56789012',
    reason: 'outperforming',
    evidence: [
      'Bench wallet outperforming Active 5 average by 15%',
      'Consistent win rate of 68% over 30 days',
      '120+ trades with stable strategy',
    ],
    urgency: 'low',
    suggestedAction: 'Consider promoting to Active 5',
    createdAt: '2026-01-08T14:00:00Z',
  },
];

const reasonLabels: Record<RecommendationReason, string> = {
  alpha_decay: 'Alpha Decay',
  martingale_pattern: 'Martingale Pattern',
  strategy_drift: 'Strategy Drift',
  honeypot_warning: 'Honeypot Warning',
  outperforming: 'Outperforming',
};

const reasonIcons: Record<RecommendationReason, React.ReactNode> = {
  alpha_decay: <TrendingDown className="h-5 w-5 text-loss" />,
  martingale_pattern: <AlertTriangle className="h-5 w-5 text-yellow-500" />,
  strategy_drift: <RefreshCw className="h-5 w-5 text-blue-500" />,
  honeypot_warning: <AlertTriangle className="h-5 w-5 text-loss" />,
  outperforming: <ArrowUpRight className="h-5 w-5 text-profit" />,
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
}: {
  recommendation: RotationRecommendation;
  onAccept: () => void;
  onDismiss: () => void;
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
                  {recommendation.walletLabel || shortenAddress(recommendation.walletAddress)}
                </p>
                <p className="text-xs text-muted-foreground font-mono">
                  {shortenAddress(recommendation.walletAddress)}
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
            <p className="text-sm font-medium">{recommendation.suggestedAction}</p>
          </div>

          {/* Actions */}
          <div className="flex items-center justify-between pt-2">
            <div className="flex items-center gap-1 text-xs text-muted-foreground">
              <Clock className="h-3 w-3" />
              {new Date(recommendation.createdAt).toLocaleString()}
            </div>
            <div className="flex gap-2">
              <Button variant="outline" size="sm" onClick={onDismiss}>
                <XCircle className="mr-1 h-4 w-4" />
                Dismiss
              </Button>
              <Button size="sm" onClick={onAccept}>
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
  const [recommendations, setRecommendations] = useState(mockRecommendations);

  const handleAccept = (rec: RotationRecommendation) => {
    if (rec.type === 'demote') {
      demoteToBench(rec.walletAddress);
      toast.success('Rotation executed', `${rec.walletLabel || shortenAddress(rec.walletAddress)} demoted to Bench`);
    } else if (rec.type === 'promote') {
      if (isRosterFull()) {
        toast.error('Roster Full', 'Demote a wallet first to make room');
        return;
      }
      promoteToActive(rec.walletAddress);
      toast.success('Rotation executed', `${shortenAddress(rec.walletAddress)} promoted to Active 5`);
    } else {
      toast.info('Alert acknowledged', 'Wallet will continue to be monitored');
    }

    setRecommendations((prev) => prev.filter((r) => r.id !== rec.id));
  };

  const handleDismiss = (id: string) => {
    setRecommendations((prev) => prev.filter((r) => r.id !== id));
    toast.info('Recommendation dismissed');
  };

  const highUrgency = recommendations.filter((r) => r.urgency === 'high');
  const otherRecommendations = recommendations.filter((r) => r.urgency !== 'high');

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
        <Button variant="outline">
          <RefreshCw className="mr-2 h-4 w-4" />
          Refresh
        </Button>
      </div>

      {/* Summary Stats */}
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

      {/* High Urgency Section */}
      {highUrgency.length > 0 && (
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
              />
            ))}
          </div>
        </div>
      )}

      {/* Other Recommendations */}
      {otherRecommendations.length > 0 && (
        <div className="space-y-4">
          <h2 className="text-xl font-semibold">Other Recommendations</h2>
          <div className="grid gap-4">
            {otherRecommendations.map((rec) => (
              <RecommendationCard
                key={rec.id}
                recommendation={rec}
                onAccept={() => handleAccept(rec)}
                onDismiss={() => handleDismiss(rec.id)}
              />
            ))}
          </div>
        </div>
      )}

      {/* Empty State */}
      {recommendations.length === 0 && (
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
