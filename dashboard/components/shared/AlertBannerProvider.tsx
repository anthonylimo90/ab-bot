'use client';

import { AlertBannerContainer } from '@/components/ui/alert-banner';
import { useNotificationStore } from '@/stores/notification-store';

export function AlertBannerProvider() {
  const { banners, removeBanner } = useNotificationStore();

  return <AlertBannerContainer banners={banners} onDismiss={removeBanner} />;
}
