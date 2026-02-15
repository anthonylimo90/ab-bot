'use client';

import { createContext, useContext, useEffect, useState } from 'react';
import { useAuthStore } from '@/stores/auth-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import { usePathname, useRouter } from 'next/navigation';
import { Button } from '@/components/ui/button';
import { AlertTriangle, RefreshCw } from 'lucide-react';

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
  const [initError, setInitError] = useState<Error | null>(null);

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
        setInitError(null);
        setHasInitialized(true);
      } catch (err) {
        setInitError(err instanceof Error ? err : new Error('Failed to initialize workspace'));
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
      setInitError(null);
    }
  }, [isAuthenticated]);

  const handleRetryInit = () => {
    setHasInitialized(false);
    setInitError(null);
  };

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

  // Show retry UI if initialization failed
  if (initError && !isExemptRoute && isAuthenticated && !isPlatformAdmin) {
    return (
      <WorkspaceContext.Provider value={contextValue}>
        <div className="flex flex-col items-center justify-center min-h-[400px] px-4">
          <div className="rounded-full bg-destructive/10 p-4 mb-4">
            <AlertTriangle className="h-8 w-8 text-destructive" />
          </div>
          <h2 className="text-xl font-semibold mb-2">Failed to load workspace</h2>
          <p className="text-muted-foreground text-center max-w-md mb-6">
            {initError.message}
          </p>
          <Button onClick={handleRetryInit}>
            <RefreshCw className="h-4 w-4 mr-2" />
            Try Again
          </Button>
        </div>
      </WorkspaceContext.Provider>
    );
  }

  return (
    <WorkspaceContext.Provider value={contextValue}>
      {children}
    </WorkspaceContext.Provider>
  );
}
