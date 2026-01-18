'use client';

import { usePathname } from 'next/navigation';
import { Header } from './Header';
import { Sidebar } from './Sidebar';
import { WorkspaceGate } from '@/components/workspace/WorkspaceGate';

// Routes that should not show the app shell (header/sidebar)
const AUTH_ROUTES = ['/login', '/signup', '/forgot-password', '/reset-password'];
const MINIMAL_LAYOUT_PREFIXES = ['/invite/'];
const ADMIN_ROUTE_PREFIX = '/admin';
// Routes that skip workspace gate (setup handles its own workspace validation)
const WORKSPACE_GATE_SKIP_ROUTES = ['/workspace/setup'];

interface AppShellProps {
  children: React.ReactNode;
}

export function AppShell({ children }: AppShellProps) {
  const pathname = usePathname();
  const isAuthRoute = AUTH_ROUTES.includes(pathname);
  const isMinimalLayoutRoute = MINIMAL_LAYOUT_PREFIXES.some(prefix => pathname.startsWith(prefix));
  const isAdminRoute = pathname.startsWith(ADMIN_ROUTE_PREFIX);
  const skipWorkspaceGate = WORKSPACE_GATE_SKIP_ROUTES.includes(pathname);

  // Auth routes, minimal layout routes, and admin routes get no trading app shell
  // Admin routes have their own layout
  if (isAuthRoute || isMinimalLayoutRoute || isAdminRoute) {
    return <>{children}</>;
  }

  // Workspace setup page skips the gate but has minimal shell
  if (skipWorkspaceGate) {
    return <>{children}</>;
  }

  // All other routes get the full app shell with workspace gate
  return (
    <WorkspaceGate>
      <div className="relative min-h-screen bg-background">
        <Header />
        <Sidebar />
        <main className="md:pl-64">
          <div className="container mx-auto p-4 md:p-6 lg:p-8">
            {children}
          </div>
        </main>
      </div>
    </WorkspaceGate>
  );
}
