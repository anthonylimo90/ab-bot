'use client';

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Progress } from '@/components/ui/progress';
import { Badge } from '@/components/ui/badge';
import { TrendingUp, TrendingDown, Shield, Target, Award, Activity } from 'lucide-react';

interface RiskComponents {
  sortino_normalized: number;
  consistency: number;
  roi_drawdown_ratio: number;
  win_rate: number;
  volatility: number;
}

interface RiskScoreDisplayProps {
  compositeScore: number;
  components: RiskComponents;
  className?: string;
}

export function RiskScoreDisplay({ compositeScore, components, className }: RiskScoreDisplayProps) {
  const scoreColor = compositeScore >= 0.7 ? 'text-profit' : compositeScore >= 0.5 ? 'text-yellow-600' : 'text-loss';
  const scoreBg = compositeScore >= 0.7 ? 'bg-profit/10' : compositeScore >= 0.5 ? 'bg-yellow-500/10' : 'bg-loss/10';

  const formatPercentage = (value: number) => `${(value * 100).toFixed(1)}%`;

  return (
    <Card className={className}>
      <CardHeader>
        <div className="flex items-center justify-between">
          <div>
            <CardTitle className="text-lg">Risk Score</CardTitle>
            <CardDescription>Composite performance metrics</CardDescription>
          </div>
          <div className={`${scoreBg} ${scoreColor} px-4 py-2 rounded-lg font-bold text-2xl`}>
            {(compositeScore * 100).toFixed(0)}
          </div>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {/* Sortino Ratio */}
        <div className="space-y-2">
          <div className="flex items-center justify-between text-sm">
            <div className="flex items-center gap-2">
              <Shield className="h-4 w-4 text-primary" />
              <span className="font-medium">Sortino Ratio</span>
              <Badge variant="outline" className="text-xs">30%</Badge>
            </div>
            <span className="text-muted-foreground">{formatPercentage(components.sortino_normalized)}</span>
          </div>
          <Progress value={components.sortino_normalized * 100} className="h-2" />
        </div>

        {/* Consistency */}
        <div className="space-y-2">
          <div className="flex items-center justify-between text-sm">
            <div className="flex items-center gap-2">
              <Target className="h-4 w-4 text-purple-600" />
              <span className="font-medium">Consistency</span>
              <Badge variant="outline" className="text-xs">25%</Badge>
            </div>
            <span className="text-muted-foreground">{formatPercentage(components.consistency)}</span>
          </div>
          <Progress value={components.consistency * 100} className="h-2" />
        </div>

        {/* ROI/MaxDD Ratio */}
        <div className="space-y-2">
          <div className="flex items-center justify-between text-sm">
            <div className="flex items-center gap-2">
              <TrendingUp className="h-4 w-4 text-profit" />
              <span className="font-medium">ROI/MaxDD</span>
              <Badge variant="outline" className="text-xs">25%</Badge>
            </div>
            <span className="text-muted-foreground">{formatPercentage(components.roi_drawdown_ratio)}</span>
          </div>
          <Progress value={components.roi_drawdown_ratio * 100} className="h-2" />
        </div>

        {/* Win Rate */}
        <div className="space-y-2">
          <div className="flex items-center justify-between text-sm">
            <div className="flex items-center gap-2">
              <Award className="h-4 w-4 text-yellow-600" />
              <span className="font-medium">Win Rate</span>
              <Badge variant="outline" className="text-xs">20%</Badge>
            </div>
            <span className="text-muted-foreground">{formatPercentage(components.win_rate)}</span>
          </div>
          <Progress value={components.win_rate * 100} className="h-2" />
        </div>

        {/* Volatility (informational) */}
        <div className="space-y-2 pt-2 border-t">
          <div className="flex items-center justify-between text-sm">
            <div className="flex items-center gap-2">
              <Activity className="h-4 w-4 text-orange-600" />
              <span className="font-medium text-muted-foreground">Volatility</span>
            </div>
            <span className="text-muted-foreground">{formatPercentage(components.volatility)}</span>
          </div>
          <p className="text-xs text-muted-foreground">
            Used for allocation scaling (lower is better)
          </p>
        </div>
      </CardContent>
    </Card>
  );
}
