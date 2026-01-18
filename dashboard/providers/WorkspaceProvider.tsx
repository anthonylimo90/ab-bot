'use client';

import { createContext, useContext, useEffect, useState } from 'react';
import { useAuthStore } from '@/stores/auth-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { usePathname, useRouter } from 'next/navigation';

interface WorkspaceContextValue {
  isInitializing: boolean;
  needsWorkspaceSelection: boolean;
  needsSetup: boolean;
}

const WorkspaceContext = createContext<WorkspaceContextValue>({
  isInitializing: true,
  needsWorkspaceSelection: false,
  needsSetup: false,
});

export function useWorkspaceContext() {
  return useContext(WorkspaceContext);
}

// Routes that don't need workspace context
const WORKSPACE_EXEMPT_ROUTES = [
  '/login',
  '/forgot-password',
  '/reset-password',
  '/admin',
  '/admin/login',
  '/admin/workspaces',
  '/admin/users',
];

const WORKSPACE_EXEMPT_PREFIXES = ['/invite/', '/admin/'];

interface WorkspaceProviderProps {
  children: React.ReactNode;
}

export function WorkspaceProvider({ children }: WorkspaceProviderProps) {
  const pathname = usePathname();
  const router = useRouter();
  const { isAuthenticated, _hasHydrated: authHydrated, user } = useAuthStore();
  const {
    currentWorkspace,
    workspaces,
    isLoading,
    _hasHydrated: workspaceHydrated,
    fetchWorkspaces,
    fetchCurrentWorkspace,
  } = useWorkspaceStore();

  const [hasInitialized, setHasInitialized] = useState(false);

  const isExemptRoute =
    WORKSPACE_EXEMPT_ROUTES.includes(pathname) ||
    WORKSPACE_EXEMPT_PREFIXES.some((prefix) => pathname.startsWith(prefix));

  const isPlatformAdmin = user?.role === 'PlatformAdmin';

  // Auto-fetch workspace data when authenticated
  useEffect(() => {
    async function initializeWorkspace() {
      if (!authHydrated || !workspaceHydrated || !isAuthenticated || isPlatformAdmin) {
        return;
      }

      if (hasInitialized) {
        return;
      }

      try {
        // Fetch workspaces list and current workspace in parallel
        await Promise.all([fetchWorkspaces(), fetchCurrentWorkspace()]);
      } finally {
        setHasInitialized(true);
      }
    }

    initializeWorkspace();
  }, [
    authHydrated,
    workspaceHydrated,
    isAuthenticated,
    isPlatformAdmin,
    hasInitialized,
    fetchWorkspaces,
    fetchCurrentWorkspace,
  ]);

  // Reset initialization when auth state changes
  useEffect(() => {
    if (!isAuthenticated) {
      setHasInitialized(false);
    }
  }, [isAuthenticated]);

  // Determine workspace state
  const isInitializing =
    !authHydrated ||
    !workspaceHydrated ||
    (isAuthenticated && !isPlatformAdmin && !hasInitialized) ||
    isLoading;

  const needsWorkspaceSelection =
    hasInitialized &&
    !currentWorkspace &&
    workspaces.length > 0 &&
    !isExemptRoute;

  const needsSetup =
    hasInitialized &&
    currentWorkspace &&
    currentWorkspace.onboarding_completed === false &&
    pathname !== '/workspace/setup' &&
    !isExemptRoute;

  // Auto-redirect to setup if needed
  useEffect(() => {
    if (needsSetup && !isLoading) {
      router.push('/workspace/setup');
    }
  }, [needsSetup, isLoading, router]);

  const contextValue: WorkspaceContextValue = {
    isInitializing,
    needsWorkspaceSelection: Boolean(needsWorkspaceSelection),
    needsSetup: Boolean(needsSetup),
  };

  return (
    <WorkspaceContext.Provider value={contextValue}>
      {children}
    </WorkspaceContext.Provider>
  );
}
