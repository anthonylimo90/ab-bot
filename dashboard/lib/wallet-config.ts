import { http, createConfig, type Config } from 'wagmi';
import { polygon, polygonAmoy } from 'wagmi/chains';
import { getDefaultConfig } from '@rainbow-me/rainbowkit';

// Default WalletConnect project ID - used as fallback
// Get from https://cloud.walletconnect.com
const DEFAULT_PROJECT_ID = process.env.NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID || 'placeholder-for-build';

// Create config with a specific project ID
export function createWalletConfig(projectId?: string): Config {
  const effectiveProjectId = projectId || DEFAULT_PROJECT_ID;

  return getDefaultConfig({
    appName: 'AB-Bot Trading',
    projectId: effectiveProjectId,
    chains: [polygon, polygonAmoy],
    transports: {
      [polygon.id]: http(),
      [polygonAmoy.id]: http(),
    },
    ssr: true,
  });
}

// Default config for static usage (e.g., during SSR or when workspace not loaded)
export const config = createWalletConfig();

// Check if a project ID is valid (not a placeholder)
export function isValidProjectId(projectId?: string | null): boolean {
  return !!projectId && projectId !== 'placeholder-for-build' && projectId.length > 10;
}

export { polygon, polygonAmoy };
