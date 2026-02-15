'use client';

import { useState, useEffect } from 'react';
import { cn } from '@/lib/utils';
import { useModeStore } from '@/stores/mode-store';
import { useDemoPortfolioStore } from '@/stores/demo-portfolio-store';
import { useWalletStore, selectHasConnectedWallet, selectPrimaryWallet } from '@/stores/wallet-store';
import { useWalletBalanceQuery } from '@/hooks/queries/useWalletsQuery';
import { useQueryClient } from '@tanstack/react-query';
import { Wallet, TestTube2, Plus, Loader2 } from 'lucide-react';
import { ConnectWalletModal } from '@/components/wallet/ConnectWalletModal';

export function ModeToggle() {
  const { mode, setMode } = useModeStore();
  const demoBalance = useDemoPortfolioStore((state) => state.balance);
  const fetchWallets = useWalletStore((state) => state.fetchWallets);
  const hasWallet = useWalletStore(selectHasConnectedWallet);
  const primaryWallet = useWalletStore(selectPrimaryWallet);
  const queryClient = useQueryClient();
  const [isHovered, setIsHovered] = useState(false);
  const [isTransitioning, setIsTransitioning] = useState(false);
  const [showConnectModal, setShowConnectModal] = useState(false);

  const isDemo = mode === 'demo';
  const { data: walletBalance } = useWalletBalanceQuery(
    !isDemo && primaryWallet ? primaryWallet.address : null
  );

  // Fetch wallets on mount when in live mode
  useEffect(() => {
    if (!isDemo) {
      fetchWallets().catch(() => {
        // Silently fail - user may not be authenticated yet
      });
    }
  }, [isDemo, fetchWallets]);

  const toggleMode = async () => {
    if (isTransitioning) return;

    setIsTransitioning(true);
    const newMode = isDemo ? 'live' : 'demo';

    try {
      // Update mode and trigger cache invalidation (now async)
      await setMode(newMode, queryClient);

      // Refetch active queries with new mode
      await queryClient.refetchQueries({
        type: 'active',
        stale: true,
      });
    } catch (error) {
      console.error('Error during mode switch:', error);
    } finally {
      setIsTransitioning(false);
    }
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
        disabled={isTransitioning}
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => setIsHovered(false)}
        className={cn(
          'relative flex items-center gap-2 rounded-full px-3 py-1.5 text-sm font-medium transition-all',
          'border',
          isTransitioning
            ? 'cursor-wait opacity-60'
            : 'cursor-pointer',
          isDemo
            ? 'bg-demo/10 text-demo border-demo/20 hover:bg-demo/20'
            : 'bg-live/10 text-live border-live/20 hover:bg-live/20'
        )}
      >
        {isTransitioning ? (
          <>
            <Loader2 className="h-4 w-4 animate-spin" />
            <span>Switching...</span>
          </>
        ) : isDemo ? (
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
            {walletBalance != null && (
              <>
                <span className="text-muted-foreground">&middot;</span>
                <span className="font-medium">{formatBalance(walletBalance.usdc_balance)}</span>
              </>
            )}
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
      {isHovered && !isTransitioning && (
        <div className="absolute top-full mt-2 left-1/2 -translate-x-1/2 px-2 py-1 bg-popover border rounded text-xs whitespace-nowrap z-50">
          Click to switch to {isDemo ? 'Live' : 'Demo'} mode
        </div>
      )}

      {/* Connect Wallet Modal */}
      <ConnectWalletModal open={showConnectModal} onOpenChange={setShowConnectModal} />
    </div>
  );
}
