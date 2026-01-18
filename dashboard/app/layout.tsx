import type { Metadata } from 'next';
import { Inter } from 'next/font/google';
import { TooltipProvider } from '@/components/ui/tooltip';
import { ToastProvider } from '@/components/shared/ToastProvider';
import { QueryProvider } from '@/providers/QueryProvider';
import { WorkspaceProvider } from '@/providers/WorkspaceProvider';
import { AuthGuard } from '@/components/auth/AuthGuard';
import { AppShell } from '@/components/layout/AppShell';
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
          <TooltipProvider>
            <AuthGuard>
              <WorkspaceProvider>
                <AppShell>{children}</AppShell>
              </WorkspaceProvider>
            </AuthGuard>
            <ToastProvider />
          </TooltipProvider>
        </QueryProvider>
      </body>
    </html>
  );
}
