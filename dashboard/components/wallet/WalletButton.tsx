'use client';

import { useState, useEffect } from 'react';
import { ConnectButton } from '@rainbow-me/rainbowkit';
import { useAccount } from 'wagmi';
import { Wallet, Loader2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useWalletAuth } from '@/hooks/useWalletAuth';

interface WalletLoginButtonProps {
  onSuccess?: () => void;
  className?: string;
}

export function WalletLoginButton({ onSuccess, className }: WalletLoginButtonProps) {
  const [isSigningIn, setIsSigningIn] = useState(false);
  const { isConnected, address } = useAccount();
  const { signIn, isLoading } = useWalletAuth();

  // When wallet connects, trigger sign-in
  useEffect(() => {
    const doSignIn = async () => {
      const success = await signIn();
      setIsSigningIn(false);
      if (success && onSuccess) {
        onSuccess();
      }
    };

    if (isConnected && address && isSigningIn) {
      doSignIn();
    }
  }, [isConnected, address, isSigningIn, signIn, onSuccess]);

  return (
    <ConnectButton.Custom>
      {({
        account,
        chain,
        openConnectModal,
        authenticationStatus,
        mounted,
      }) => {
        const ready = mounted && authenticationStatus !== 'loading';
        const connected = ready && account && chain;

        return (
          <div
            {...(!ready && {
              'aria-hidden': true,
              style: {
                opacity: 0,
                pointerEvents: 'none',
                userSelect: 'none',
              },
            })}
          >
            {(() => {
              if (!connected) {
                return (
                  <Button
                    variant="outline"
                    className={className}
                    onClick={() => {
                      setIsSigningIn(true);
                      openConnectModal();
                    }}
                    disabled={isLoading}
                  >
                    {isLoading ? (
                      <>
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                        Connecting...
                      </>
                    ) : (
                      <>
                        <Wallet className="mr-2 h-4 w-4" />
                        Connect Wallet
                      </>
                    )}
                  </Button>
                );
              }

              // Wallet is connected but not signed in yet
              if (isSigningIn || isLoading) {
                return (
                  <Button variant="outline" className={className} disabled>
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                    Signing in...
                  </Button>
                );
              }

              // Wallet connected - prompt to sign in
              return (
                <Button
                  variant="outline"
                  className={className}
                  onClick={() => setIsSigningIn(true)}
                  disabled={isLoading}
                >
                  <Wallet className="mr-2 h-4 w-4" />
                  Sign in with {account.displayName}
                </Button>
              );
            })()}
          </div>
        );
      }}
    </ConnectButton.Custom>
  );
}

interface WalletStatusButtonProps {
  className?: string;
}

export function WalletStatusButton({ className }: WalletStatusButtonProps) {
  return (
    <ConnectButton.Custom>
      {({
        account,
        chain,
        openAccountModal,
        openChainModal,
        openConnectModal,
        authenticationStatus,
        mounted,
      }) => {
        const ready = mounted && authenticationStatus !== 'loading';
        const connected = ready && account && chain;

        return (
          <div
            {...(!ready && {
              'aria-hidden': true,
              style: {
                opacity: 0,
                pointerEvents: 'none',
                userSelect: 'none',
              },
            })}
          >
            {(() => {
              if (!connected) {
                return (
                  <Button
                    variant="ghost"
                    size="sm"
                    className={className}
                    onClick={openConnectModal}
                  >
                    <Wallet className="mr-2 h-4 w-4" />
                    Connect
                  </Button>
                );
              }

              if (chain.unsupported) {
                return (
                  <Button
                    variant="destructive"
                    size="sm"
                    onClick={openChainModal}
                  >
                    Wrong network
                  </Button>
                );
              }

              return (
                <Button
                  variant="ghost"
                  size="sm"
                  className={className}
                  onClick={openAccountModal}
                >
                  <Wallet className="mr-2 h-4 w-4" />
                  {account.displayName}
                </Button>
              );
            })()}
          </div>
        );
      }}
    </ConnectButton.Custom>
  );
}
