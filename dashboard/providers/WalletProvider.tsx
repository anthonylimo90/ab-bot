'use client';

import { useState, useEffect, useMemo } from 'react';
import { WagmiProvider, type State } from 'wagmi';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RainbowKitProvider, darkTheme } from '@rainbow-me/rainbowkit';
import { createWalletConfig, isValidProjectId } from '@/lib/wallet-config';
import { useWorkspaceStore } from '@/stores/workspace-store';

import '@rainbow-me/rainbowkit/styles.css';

interface WalletProviderProps {
  children: React.ReactNode;
}

// Create a stable query client
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 60 * 1000, // 1 minute
    },
  },
});

export function WalletProvider({ children }: WalletProviderProps) {
  const [mounted, setMounted] = useState(false);
  const { currentWorkspace, _hasHydrated } = useWorkspaceStore();

  // Get project ID from workspace or fallback to env var
  const projectId = currentWorkspace?.walletconnect_project_id;
  const hasValidProjectId = isValidProjectId(projectId);

  // Create wagmi config based on workspace project ID
  // Memoize to avoid recreating on every render
  const config = useMemo(() => {
    return createWalletConfig(projectId || undefined);
  }, [projectId]);

  useEffect(() => {
    setMounted(true);
  }, []);

  // Wait for both client-side mount and store hydration
  if (!mounted || !_hasHydrated) {
    return (
      <QueryClientProvider client={queryClient}>
        {children}
      </QueryClientProvider>
    );
  }

  return (
    <WagmiProvider config={config}>
      <QueryClientProvider client={queryClient}>
        <RainbowKitProvider
          theme={darkTheme({
            accentColor: '#10b981',
            accentColorForeground: 'white',
            borderRadius: 'medium',
          })}
          modalSize="compact"
        >
          {children}
        </RainbowKitProvider>
      </QueryClientProvider>
    </WagmiProvider>
  );
}

// Hook to check if wallet connection is properly configured
export function useWalletConfigured(): boolean {
  const { currentWorkspace } = useWorkspaceStore();
  const projectId = currentWorkspace?.walletconnect_project_id;
  return isValidProjectId(projectId) || isValidProjectId(process.env.NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID);
}
