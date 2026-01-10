'use client';

import { useState, useEffect } from 'react';
import { cn } from '@/lib/utils';
import { useModeStore } from '@/stores/mode-store';
import { useDemoPortfolioStore } from '@/stores/demo-portfolio-store';
import { useWalletStore, selectHasConnectedWallet, selectPrimaryWallet } from '@/stores/wallet-store';
import { Wallet, TestTube2, Plus } from 'lucide-react';
import { ConnectWalletModal } from '@/components/wallet/ConnectWalletModal';

export function ModeToggle() {
  const { mode, setMode } = useModeStore();
  const demoBalance = useDemoPortfolioStore((state) => state.balance);
  const fetchWallets = useWalletStore((state) => state.fetchWallets);
  const hasWallet = useWalletStore(selectHasConnectedWallet);
  const primaryWallet = useWalletStore(selectPrimaryWallet);
  const [isHovered, setIsHovered] = useState(false);
  const [showConnectModal, setShowConnectModal] = useState(false);

  const isDemo = mode === 'demo';

  // Fetch wallets on mount when in live mode
  useEffect(() => {
    if (!isDemo) {
      fetchWallets().catch(() => {
        // Silently fail - user may not be authenticated yet
      });
    }
  }, [isDemo, fetchWallets]);

  const toggleMode = () => {
    setMode(isDemo ? 'live' : 'demo');
  };

  const formatBalance = (balance: number) => {
    return new Intl.NumberFormat('en-US', {
      style: 'currency',
      currency: 'USD',
      minimumFractionDigits: 0,
      maximumFractionDigits: 0,
    }).format(balance);
  };

  const truncateAddress = (address: string) => {
    return `${address.slice(0, 6)}...${address.slice(-4)}`;
  };

  return (
    <div className="flex items-center gap-2">
      {/* Mode Toggle Button */}
      <button
        onClick={toggleMode}
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => setIsHovered(false)}
        className={cn(
          'relative flex items-center gap-2 rounded-full px-3 py-1.5 text-sm font-medium transition-all',
          'border cursor-pointer',
          isDemo
            ? 'bg-demo/10 text-demo border-demo/20 hover:bg-demo/20'
            : 'bg-live/10 text-live border-live/20 hover:bg-live/20'
        )}
      >
        {isDemo ? (
          <>
            <TestTube2 className="h-4 w-4" />
            <span>Demo</span>
          </>
        ) : (
          <>
            <span className="h-2 w-2 rounded-full animate-pulse bg-live" />
            <span>Live</span>
          </>
        )}
      </button>

      {/* Demo Balance Display */}
      {isDemo && (
        <div className="hidden sm:flex items-center gap-1.5 px-3 py-1.5 rounded-full bg-muted text-sm">
          <Wallet className="h-4 w-4 text-muted-foreground" />
          <span className="font-medium">{formatBalance(demoBalance)}</span>
        </div>
      )}

      {/* Live Mode: Show wallet info or connect button */}
      {!isDemo && (
        hasWallet && primaryWallet ? (
          <div className="hidden sm:flex items-center gap-1.5 px-3 py-1.5 rounded-full bg-muted text-sm">
            <Wallet className="h-4 w-4 text-muted-foreground" />
            <span className="font-mono text-xs">
              {primaryWallet.label || truncateAddress(primaryWallet.address)}
            </span>
          </div>
        ) : (
          <button
            onClick={() => setShowConnectModal(true)}
            className="hidden sm:flex items-center gap-1.5 px-3 py-1.5 rounded-full bg-primary/10 text-primary border border-primary/20 hover:bg-primary/20 text-sm font-medium transition-colors"
          >
            <Plus className="h-4 w-4" />
            <span>Connect</span>
          </button>
        )
      )}

      {/* Hover tooltip */}
      {isHovered && (
        <div className="absolute top-full mt-2 left-1/2 -translate-x-1/2 px-2 py-1 bg-popover border rounded text-xs whitespace-nowrap z-50">
          Click to switch to {isDemo ? 'Live' : 'Demo'} mode
        </div>
      )}

      {/* Connect Wallet Modal */}
      <ConnectWalletModal open={showConnectModal} onOpenChange={setShowConnectModal} />
    </div>
  );
}
