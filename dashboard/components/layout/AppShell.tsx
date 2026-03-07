'use client';

import { usePathname } from 'next/navigation';
import { Header } from './Header';
import { Sidebar } from './Sidebar';
import { WorkspaceGate } from '@/components/workspace/WorkspaceGate';
import { ActivityProvider } from '@/hooks/useActivity';

// Routes that should not show the app shell (header/sidebar)
const AUTH_ROUTES = ['/login', '/signup', '/forgot-password', '/reset-password'];
const MINIMAL_LAYOUT_PREFIXES = ['/invite/'];
const ADMIN_ROUTE_PREFIX = '/admin';

interface AppShellProps {
  children: React.ReactNode;
}

export function AppShell({ children }: AppShellProps) {
  const pathname = usePathname();
  const isAuthRoute = AUTH_ROUTES.includes(pathname);
  const isMinimalLayoutRoute = MINIMAL_LAYOUT_PREFIXES.some(prefix => pathname.startsWith(prefix));
  const isAdminRoute = pathname.startsWith(ADMIN_ROUTE_PREFIX);

  // Auth routes, minimal layout routes, and admin routes get no trading app shell
  // Admin routes have their own layout
  if (isAuthRoute || isMinimalLayoutRoute || isAdminRoute) {
    return <>{children}</>;
  }

  // All other routes get the full app shell with workspace gate
  return (
    <WorkspaceGate>
      <ActivityProvider>
        <div className="relative min-h-screen bg-background">
          <Header />
          <Sidebar />
          <main className="min-w-0 md:pl-64">
            <div className="mx-auto w-full max-w-screen-2xl px-4 py-4 md:px-6 md:py-6 lg:px-8 lg:py-8">
              {children}
            </div>
          </main>
        </div>
      </ActivityProvider>
    </WorkspaceGate>
  );
}
