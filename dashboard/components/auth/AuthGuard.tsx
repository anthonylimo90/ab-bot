'use client';

import { useEffect, useState } from 'react';
import { useRouter, usePathname } from 'next/navigation';
import { useAuthStore } from '@/stores/auth-store';

// Routes that don't require authentication
const PUBLIC_ROUTES = ['/login', '/forgot-password', '/reset-password'];

interface AuthGuardProps {
  children: React.ReactNode;
}

export function AuthGuard({ children }: AuthGuardProps) {
  const router = useRouter();
  const pathname = usePathname();
  const { isAuthenticated, isLoading, checkAuth, _hasHydrated } = useAuthStore();
  const [hasChecked, setHasChecked] = useState(false);
  const [mounted, setMounted] = useState(false);

  const isPublicRoute = PUBLIC_ROUTES.includes(pathname);

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

  // Redirect to login if not authenticated (after hydration and auth check)
  // Skip redirect for public routes
  useEffect(() => {
    if (mounted && _hasHydrated && hasChecked && !isLoading && !isAuthenticated && !isPublicRoute) {
      router.push(`/login?redirect=${encodeURIComponent(pathname)}`);
    }
  }, [mounted, _hasHydrated, hasChecked, isAuthenticated, isLoading, pathname, router, isPublicRoute]);

  // Redirect authenticated users away from auth pages
  useEffect(() => {
    if (mounted && _hasHydrated && hasChecked && !isLoading && isAuthenticated && isPublicRoute) {
      router.push('/');
    }
  }, [mounted, _hasHydrated, hasChecked, isAuthenticated, isLoading, isPublicRoute, router]);

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
