'use client';

import { AlertCircle, RefreshCw, WifiOff, ServerCrash } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

interface ErrorDisplayProps {
  error: Error | { message: string } | string;
  onRetry?: () => void;
  isRetrying?: boolean;
  title?: string;
  className?: string;
  variant?: 'inline' | 'full-page' | 'card';
}

export function ErrorDisplay({
  error,
  onRetry,
  isRetrying = false,
  title,
  className,
  variant = 'card',
}: ErrorDisplayProps) {
  const message = typeof error === 'string' ? error : error.message;

  // Determine error type for appropriate icon
  const isNetworkError =
    message.toLowerCase().includes('network') ||
    message.toLowerCase().includes('fetch') ||
    message.toLowerCase().includes('failed to load');
  const isServerError =
    message.toLowerCase().includes('500') ||
    message.toLowerCase().includes('server') ||
    message.toLowerCase().includes('internal');

  const Icon = isNetworkError ? WifiOff : isServerError ? ServerCrash : AlertCircle;

  const defaultTitle = isNetworkError
    ? 'Connection Error'
    : isServerError
    ? 'Server Error'
    : 'Something went wrong';

  if (variant === 'inline') {
    return (
      <div
        className={cn(
          'flex items-center gap-2 text-sm text-destructive',
          className
        )}
        role="alert"
      >
        <AlertCircle className="h-4 w-4 flex-shrink-0" />
        <span>{message}</span>
        {onRetry && (
          <Button
            variant="ghost"
            size="sm"
            className="h-6 px-2"
            onClick={onRetry}
            disabled={isRetrying}
          >
            {isRetrying ? (
              <RefreshCw className="h-3 w-3 animate-spin" />
            ) : (
              'Retry'
            )}
          </Button>
        )}
      </div>
    );
  }

  if (variant === 'full-page') {
    return (
      <div
        className={cn(
          'flex flex-col items-center justify-center min-h-[400px] px-4',
          className
        )}
        role="alert"
      >
        <div className="rounded-full bg-destructive/10 p-4 mb-4">
          <Icon className="h-8 w-8 text-destructive" />
        </div>
        <h2 className="text-xl font-semibold mb-2">{title || defaultTitle}</h2>
        <p className="text-muted-foreground text-center max-w-md mb-6">
          {message}
        </p>
        {onRetry && (
          <Button onClick={onRetry} disabled={isRetrying}>
            {isRetrying ? (
              <>
                <RefreshCw className="h-4 w-4 mr-2 animate-spin" />
                Retrying...
              </>
            ) : (
              <>
                <RefreshCw className="h-4 w-4 mr-2" />
                Try Again
              </>
            )}
          </Button>
        )}
      </div>
    );
  }

  // Default: card variant
  return (
    <div
      className={cn(
        'rounded-lg border border-destructive/20 bg-destructive/5 p-4',
        className
      )}
      role="alert"
    >
      <div className="flex items-start gap-3">
        <div className="rounded-full bg-destructive/10 p-2 flex-shrink-0">
          <Icon className="h-4 w-4 text-destructive" />
        </div>
        <div className="flex-1 min-w-0">
          <h3 className="font-medium text-destructive">
            {title || defaultTitle}
          </h3>
          <p className="text-sm text-muted-foreground mt-1">{message}</p>
          {onRetry && (
            <Button
              variant="outline"
              size="sm"
              className="mt-3"
              onClick={onRetry}
              disabled={isRetrying}
            >
              {isRetrying ? (
                <>
                  <RefreshCw className="h-3 w-3 mr-2 animate-spin" />
                  Retrying...
                </>
              ) : (
                <>
                  <RefreshCw className="h-3 w-3 mr-2" />
                  Try Again
                </>
              )}
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}

// Empty state component for when data is missing
interface EmptyStateProps {
  icon?: React.ReactNode;
  title: string;
  description?: string;
  action?: {
    label: string;
    onClick: () => void;
  };
  className?: string;
}

export function EmptyState({
  icon,
  title,
  description,
  action,
  className,
}: EmptyStateProps) {
  return (
    <div
      className={cn(
        'flex flex-col items-center justify-center py-12 px-4',
        className
      )}
    >
      {icon && (
        <div className="rounded-full bg-muted p-4 mb-4">{icon}</div>
      )}
      <h3 className="text-lg font-medium mb-1">{title}</h3>
      {description && (
        <p className="text-sm text-muted-foreground text-center max-w-sm mb-4">
          {description}
        </p>
      )}
      {action && (
        <Button variant="outline" onClick={action.onClick}>
          {action.label}
        </Button>
      )}
    </div>
  );
}
