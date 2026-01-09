'use client';

import { useToastStore } from '@/stores/toast-store';
import { ToastContainer } from '@/components/ui/toast';

export function ToastProvider() {
  const { toasts, removeToast } = useToastStore();

  return <ToastContainer toasts={toasts} onClose={removeToast} />;
}
