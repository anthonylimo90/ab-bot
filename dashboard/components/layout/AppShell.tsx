'use client';

import { usePathname } from 'next/navigation';
import { Header } from './Header';
import { Sidebar } from './Sidebar';

// Routes that should not show the app shell (header/sidebar)
const AUTH_ROUTES = ['/login', '/signup', '/forgot-password', '/reset-password'];
const MINIMAL_LAYOUT_PREFIXES = ['/invite/'];

interface AppShellProps {
  children: React.ReactNode;
}

export function AppShell({ children }: AppShellProps) {
  const pathname = usePathname();
  const isAuthRoute = AUTH_ROUTES.includes(pathname);
  const isMinimalLayoutRoute = MINIMAL_LAYOUT_PREFIXES.some(prefix => pathname.startsWith(prefix));

  // Auth routes and minimal layout routes get no app shell
  if (isAuthRoute || isMinimalLayoutRoute) {
    return <>{children}</>;
  }

  // All other routes get the full app shell
  return (
    <div className="relative min-h-screen bg-background">
      <Header />
      <Sidebar />
      <main className="md:pl-64">
        <div className="container mx-auto p-4 md:p-6 lg:p-8">
          {children}
        </div>
      </main>
    </div>
  );
}
