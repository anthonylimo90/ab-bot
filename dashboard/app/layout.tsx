import type { Metadata } from 'next';
import { Inter } from 'next/font/google';
import { TooltipProvider } from '@/components/ui/tooltip';
import { ToastProvider } from '@/components/shared/ToastProvider';
import { AlertBannerProvider } from '@/components/shared/AlertBannerProvider';
import { QueryProvider } from '@/providers/QueryProvider';
import { WalletProvider } from '@/providers/WalletProvider';
import { WorkspaceProvider } from '@/providers/WorkspaceProvider';
import { AuthGuard } from '@/components/auth/AuthGuard';
import { AppShell } from '@/components/layout/AppShell';
import { WebSocketProvider } from '@/providers/WebSocketProvider';
import './globals.css';

const inter = Inter({ subsets: ['latin'] });

export const metadata: Metadata = {
  title: 'AB-Bot | Polymarket Trading Dashboard',
  description: 'Automated trading and copy trading for Polymarket',
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className={inter.className}>
        <QueryProvider>
          <WalletProvider>
            <TooltipProvider>
              <AuthGuard>
                <WorkspaceProvider>
                  <WebSocketProvider>
                    <AppShell>{children}</AppShell>
                  </WebSocketProvider>
                </WorkspaceProvider>
              </AuthGuard>
              <ToastProvider />
              <AlertBannerProvider />
            </TooltipProvider>
          </WalletProvider>
        </QueryProvider>
      </body>
    </html>
  );
}
