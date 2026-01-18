'use client';

import { useState } from 'react';
import { cn } from '@/lib/utils';
import { AlertTriangle, X, Ban, Clock, CheckCircle } from 'lucide-react';
import { Button } from './button';

export type AlertBannerType = 'warning' | 'error' | 'info' | 'success';

export interface AlertBanner {
  id: string;
  type: AlertBannerType;
  title: string;
  description?: string;
  dismissible?: boolean;
  action?: {
    label: string;
    onClick: () => void;
  };
}

interface AlertBannerItemProps extends AlertBanner {
  onDismiss: (id: string) => void;
}

const bannerIcons: Record<AlertBannerType, React.ReactNode> = {
  warning: <AlertTriangle className="h-5 w-5" />,
  error: <Ban className="h-5 w-5" />,
  info: <Clock className="h-5 w-5" />,
  success: <CheckCircle className="h-5 w-5" />,
};

const bannerStyles: Record<AlertBannerType, string> = {
  warning: 'bg-yellow-500/10 border-yellow-500/20 text-yellow-500',
  error: 'bg-red-500/10 border-red-500/20 text-red-500',
  info: 'bg-blue-500/10 border-blue-500/20 text-blue-500',
  success: 'bg-green-500/10 border-green-500/20 text-green-500',
};

export function AlertBannerItem({
  id,
  type,
  title,
  description,
  dismissible = true,
  action,
  onDismiss,
}: AlertBannerItemProps) {
  return (
    <div
      className={cn(
        'flex items-center justify-between px-4 py-3 border-b',
        bannerStyles[type]
      )}
    >
      <div className="flex items-center gap-3">
        {bannerIcons[type]}
        <div>
          <p className="text-sm font-medium">{title}</p>
          {description && (
            <p className="text-sm opacity-80">{description}</p>
          )}
        </div>
      </div>
      <div className="flex items-center gap-2">
        {action && (
          <Button
            variant="outline"
            size="sm"
            onClick={action.onClick}
            className={cn(
              'border-current/50 hover:bg-current/10',
              type === 'error' && 'text-red-500 hover:text-red-600',
              type === 'warning' && 'text-yellow-500 hover:text-yellow-600',
              type === 'info' && 'text-blue-500 hover:text-blue-600',
              type === 'success' && 'text-green-500 hover:text-green-600'
            )}
          >
            {action.label}
          </Button>
        )}
        {dismissible && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onDismiss(id)}
            className="h-8 w-8 p-0 hover:bg-current/10"
          >
            <X className="h-4 w-4" />
          </Button>
        )}
      </div>
    </div>
  );
}

interface AlertBannerContainerProps {
  banners: AlertBanner[];
  onDismiss: (id: string) => void;
}

export function AlertBannerContainer({ banners, onDismiss }: AlertBannerContainerProps) {
  if (banners.length === 0) return null;

  return (
    <div className="fixed top-0 left-0 right-0 z-50">
      {banners.map((banner) => (
        <AlertBannerItem key={banner.id} {...banner} onDismiss={onDismiss} />
      ))}
    </div>
  );
}
