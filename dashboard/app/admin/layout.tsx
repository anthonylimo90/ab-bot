'use client';

import { useEffect } from 'react';
import { useRouter, usePathname } from 'next/navigation';
import { AdminHeader } from '@/components/admin/AdminHeader';
import { AdminSidebar } from '@/components/admin/AdminSidebar';
import { useAuthStore } from '@/stores/auth-store';

interface AdminLayoutProps {
  children: React.ReactNode;
}

export default function AdminLayout({ children }: AdminLayoutProps) {
  const router = useRouter();
  const pathname = usePathname();
  const { user, isAuthenticated, isLoading } = useAuthStore();

  const isAdminLoginRoute = pathname === '/admin/login';

  // Redirect non-admin users (must be called before any early returns to satisfy React hooks rules)
  useEffect(() => {
    if (!isAdminLoginRoute && !isLoading && isAuthenticated && user?.role !== 'Admin') {
      router.push('/');
    }
  }, [isAdminLoginRoute, isLoading, isAuthenticated, user, router]);

  // Admin login page should not have the admin shell
  if (isAdminLoginRoute) {
    return <>{children}</>;
  }

  // Show loading while checking auth
  if (isLoading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <div className="flex flex-col items-center gap-4">
          <div className="h-8 w-8 animate-spin rounded-full border-4 border-primary border-t-transparent" />
          <p className="text-sm text-muted-foreground">Loading...</p>
        </div>
      </div>
    );
  }

  // Don't render for non-admin users
  if (!isAuthenticated || user?.role !== 'Admin') {
    return null;
  }

  return (
    <div className="relative min-h-screen bg-background">
      <AdminHeader />
      <AdminSidebar />
      <main className="md:pl-64">
        <div className="container mx-auto p-4 md:p-6 lg:p-8">
          {children}
        </div>
      </main>
    </div>
  );
}
