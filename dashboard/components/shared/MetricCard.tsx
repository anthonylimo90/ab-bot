import { memo } from 'react';
import { cn } from '@/lib/utils';
import { Card, CardContent } from '@/components/ui/card';
import { TrendingUp, TrendingDown, Minus } from 'lucide-react';

interface MetricCardProps {
  title: string;
  value: string;
  change?: number;
  changeLabel?: string;
  trend?: 'up' | 'down' | 'neutral';
  className?: string;
}

export const MetricCard = memo(function MetricCard({
  title,
  value,
  change,
  changeLabel,
  trend,
  className,
}: MetricCardProps) {
  const getTrendIcon = () => {
    switch (trend) {
      case 'up':
        return <TrendingUp className="h-4 w-4 text-profit" />;
      case 'down':
        return <TrendingDown className="h-4 w-4 text-loss" />;
      default:
        return <Minus className="h-4 w-4 text-muted-foreground" />;
    }
  };

  const getTrendColor = () => {
    switch (trend) {
      case 'up':
        return 'text-profit';
      case 'down':
        return 'text-loss';
      default:
        return 'text-muted-foreground';
    }
  };

  return (
    <Card className={cn('', className)}>
      <CardContent className="p-6">
        <div className="flex flex-col gap-1">
          <span className="text-sm font-medium text-muted-foreground">
            {title}
          </span>
          <div className="flex items-baseline gap-2">
            <span className="text-2xl font-bold tabular-nums">{value}</span>
            {change !== undefined && (
              <div className="flex items-center gap-1">
                {getTrendIcon()}
                <span className={cn('text-sm tabular-nums', getTrendColor())}>
                  {change > 0 ? '+' : ''}
                  {change.toFixed(1)}%
                </span>
              </div>
            )}
          </div>
          {changeLabel && (
            <span className="text-xs text-muted-foreground">{changeLabel}</span>
          )}
        </div>
      </CardContent>
    </Card>
  );
});
