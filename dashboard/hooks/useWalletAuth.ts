'use client';

import { useState, useCallback } from 'react';
import { useAccount, useSignMessage, useDisconnect } from 'wagmi';
import { SiweMessage } from 'siwe';
import api from '@/lib/api';
import { useAuthStore } from '@/stores/auth-store';
import { useToastStore } from '@/stores/toast-store';
import type { User } from '@/types/api';

interface UseWalletAuthReturn {
  isLoading: boolean;
  error: string | null;
  signIn: () => Promise<boolean>;
  linkWallet: () => Promise<boolean>;
  isConnected: boolean;
  address: string | undefined;
}

export function useWalletAuth(): UseWalletAuthReturn {
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { address, isConnected } = useAccount();
  const { signMessageAsync } = useSignMessage();
  const { disconnect } = useDisconnect();

  const setAuth = useAuthStore((state) => state.setAuth);
  const addToast = useToastStore((state) => state.addToast);

  const signIn = useCallback(async (): Promise<boolean> => {
    if (!address) {
      setError('No wallet connected');
      return false;
    }

    setIsLoading(true);
    setError(null);

    try {
      // Step 1: Get challenge from server
      const { message: siweMessage, nonce } = await api.walletChallenge(address);

      // Step 2: Sign the message with MetaMask
      const signature = await signMessageAsync({ message: siweMessage });

      // Step 3: Verify signature and get JWT
      const { token, user, is_new_user } = await api.walletVerify(
        siweMessage,
        signature
      );

      // Convert WalletUser to User format for auth store
      const authUser: User = {
        id: user.id,
        email: user.email,
        name: user.name,
        wallet_address: user.wallet_address,
        role: user.role as 'Viewer' | 'Trader' | 'PlatformAdmin',
        created_at: user.created_at,
      };

      setAuth(token, authUser);

      addToast({
        type: 'success',
        title: is_new_user ? 'Account created!' : 'Welcome back!',
        description: `Signed in with wallet ${address.slice(0, 6)}...${address.slice(-4)}`,
      });

      return true;
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Wallet sign-in failed';
      setError(message);
      addToast({
        type: 'error',
        title: 'Sign-in failed',
        description: message,
      });
      return false;
    } finally {
      setIsLoading(false);
    }
  }, [address, signMessageAsync, setAuth, addToast]);

  const linkWallet = useCallback(async (): Promise<boolean> => {
    if (!address) {
      setError('No wallet connected');
      return false;
    }

    setIsLoading(true);
    setError(null);

    try {
      // Step 1: Get challenge from server
      const { message: siweMessage } = await api.walletChallenge(address);

      // Step 2: Sign the message with MetaMask
      const signature = await signMessageAsync({ message: siweMessage });

      // Step 3: Link wallet to existing account
      const result = await api.walletLink(siweMessage, signature);

      addToast({
        type: 'success',
        title: 'Wallet linked!',
        description: `Connected ${result.wallet_address.slice(0, 6)}...${result.wallet_address.slice(-4)}`,
      });

      return true;
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to link wallet';
      setError(message);
      addToast({
        type: 'error',
        title: 'Link failed',
        description: message,
      });
      return false;
    } finally {
      setIsLoading(false);
    }
  }, [address, signMessageAsync, addToast]);

  return {
    isLoading,
    error,
    signIn,
    linkWallet,
    isConnected,
    address,
  };
}
