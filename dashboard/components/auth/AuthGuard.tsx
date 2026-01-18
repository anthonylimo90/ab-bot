'use client';

import { useEffect, useState } from 'react';
import { useRouter, usePathname } from 'next/navigation';
import { useAuthStore } from '@/stores/auth-store';

// Routes that don't require authentication
const PUBLIC_ROUTES = ['/login', '/forgot-password', '/reset-password', '/admin/login'];
const PUBLIC_ROUTE_PREFIXES = ['/invite/'];

// Admin routes prefix
const ADMIN_ROUTE_PREFIX = '/admin';

interface AuthGuardProps {
  children: React.ReactNode;
}

export function AuthGuard({ children }: AuthGuardProps) {
  const router = useRouter();
  const pathname = usePathname();
  const { isAuthenticated, isLoading, checkAuth, _hasHydrated, user } = useAuthStore();
  const [hasChecked, setHasChecked] = useState(false);
  const [mounted, setMounted] = useState(false);

  const isPublicRoute = PUBLIC_ROUTES.includes(pathname) ||
    PUBLIC_ROUTE_PREFIXES.some(prefix => pathname.startsWith(prefix));
  const isAdminRoute = pathname.startsWith(ADMIN_ROUTE_PREFIX);
  const isAdminLoginRoute = pathname === '/admin/login';
  const isUserLoginRoute = pathname === '/login';
  const isPlatformAdmin = user?.role === 'Admin';

  // Track client-side mount
  useEffect(() => {
    setMounted(true);
  }, []);

  // Wait for hydration, then check auth
  useEffect(() => {
    if (mounted && _hasHydrated && !hasChecked) {
      checkAuth().then(() => setHasChecked(true));
    }
  }, [mounted, _hasHydrated, hasChecked, checkAuth]);

  // Redirect to appropriate login if not authenticated (after hydration and auth check)
  // Skip redirect for public routes
  useEffect(() => {
    if (mounted && _hasHydrated && hasChecked && !isLoading && !isAuthenticated && !isPublicRoute) {
      // Non-authenticated users trying to access admin routes go to admin login
      if (isAdminRoute) {
        router.push('/admin/login');
      } else {
        router.push(`/login?redirect=${encodeURIComponent(pathname)}`);
      }
    }
  }, [mounted, _hasHydrated, hasChecked, isAuthenticated, isLoading, pathname, router, isPublicRoute, isAdminRoute]);

  // Role-based route protection for authenticated users
  useEffect(() => {
    if (mounted && _hasHydrated && hasChecked && !isLoading && isAuthenticated) {
      // Admin on trading routes (not admin routes) -> redirect to admin portal
      if (isPlatformAdmin && !isAdminRoute && !isPublicRoute) {
        router.push('/admin/workspaces');
        return;
      }

      // Non-admin on admin routes (except admin login) -> redirect to trading dashboard
      if (!isPlatformAdmin && isAdminRoute && !isAdminLoginRoute) {
        router.push('/');
        return;
      }

      // Admin on user login page -> redirect to admin login
      if (isPlatformAdmin && isUserLoginRoute) {
        router.push('/admin/login');
        return;
      }

      // Non-admin on admin login page -> redirect to user login
      if (!isPlatformAdmin && isAdminLoginRoute) {
        router.push('/login');
        return;
      }

      // Authenticated admin on admin login -> redirect to admin portal
      if (isPlatformAdmin && isAdminLoginRoute) {
        router.push('/admin/workspaces');
        return;
      }

      // Authenticated non-admin on user login -> redirect to dashboard
      if (!isPlatformAdmin && isUserLoginRoute) {
        router.push('/');
        return;
      }
    }
  }, [mounted, _hasHydrated, hasChecked, isAuthenticated, isLoading, isPlatformAdmin, isAdminRoute, isAdminLoginRoute, isUserLoginRoute, isPublicRoute, router]);

  // For public routes, render children immediately (no auth check needed to view)
  if (isPublicRoute) {
    return <>{children}</>;
  }

  // Show loading while not mounted, hydrating, or checking auth for protected routes
  if (!mounted || !_hasHydrated || !hasChecked || isLoading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <div className="flex flex-col items-center gap-4">
          <div className="h-8 w-8 animate-spin rounded-full border-4 border-primary border-t-transparent" />
          <p className="text-sm text-muted-foreground">Loading...</p>
        </div>
      </div>
    );
  }

  if (!isAuthenticated) {
    return null;
  }

  return <>{children}</>;
}
