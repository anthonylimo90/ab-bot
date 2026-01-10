'use client';

import { useState } from 'react';
import { Eye, EyeOff, Wallet, AlertTriangle, Shield, Loader2 } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { useWalletStore } from '@/stores/wallet-store';
import { cn } from '@/lib/utils';

interface ConnectWalletModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function ConnectWalletModal({ open, onOpenChange }: ConnectWalletModalProps) {
  const [address, setAddress] = useState('');
  const [privateKey, setPrivateKey] = useState('');
  const [label, setLabel] = useState('');
  const [showPrivateKey, setShowPrivateKey] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { connectWallet, isLoading } = useWalletStore();

  const resetForm = () => {
    setAddress('');
    setPrivateKey('');
    setLabel('');
    setError(null);
    setShowPrivateKey(false);
  };

  const handleClose = () => {
    resetForm();
    onOpenChange(false);
  };

  const validateAddress = (addr: string): boolean => {
    // Basic Ethereum address validation
    return /^0x[a-fA-F0-9]{40}$/.test(addr);
  };

  const validatePrivateKey = (key: string): boolean => {
    // Private key should be 64 hex chars, optionally with 0x prefix
    const cleanKey = key.startsWith('0x') ? key.slice(2) : key;
    return /^[a-fA-F0-9]{64}$/.test(cleanKey);
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);

    // Validate address
    if (!validateAddress(address)) {
      setError('Please enter a valid Ethereum address (0x...)');
      return;
    }

    // Validate private key
    if (!validatePrivateKey(privateKey)) {
      setError('Please enter a valid private key (64 hex characters)');
      return;
    }

    try {
      await connectWallet(address, privateKey, label || undefined);
      handleClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to connect wallet');
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Wallet className="h-5 w-5" />
            Connect Wallet
          </DialogTitle>
          <DialogDescription>
            Connect your wallet to enable live trading. Your private key is encrypted and stored securely.
          </DialogDescription>
        </DialogHeader>

        <form onSubmit={handleSubmit} className="space-y-4">
          {/* Security Notice */}
          <div className="flex items-start gap-2 p-3 rounded-lg bg-amber-500/10 border border-amber-500/20">
            <Shield className="h-5 w-5 text-amber-500 shrink-0 mt-0.5" />
            <div className="text-sm">
              <p className="font-medium text-amber-500">Security Notice</p>
              <p className="text-muted-foreground">
                Your private key is encrypted with AES-256 before storage. Never share your private key with anyone.
              </p>
            </div>
          </div>

          {/* Wallet Address */}
          <div className="space-y-2">
            <Label htmlFor="address">Wallet Address</Label>
            <Input
              id="address"
              type="text"
              placeholder="0x..."
              value={address}
              onChange={(e) => setAddress(e.target.value)}
              className="font-mono"
              disabled={isLoading}
            />
          </div>

          {/* Private Key */}
          <div className="space-y-2">
            <Label htmlFor="privateKey">Private Key</Label>
            <div className="relative">
              <Input
                id="privateKey"
                type={showPrivateKey ? 'text' : 'password'}
                placeholder="Enter your private key"
                value={privateKey}
                onChange={(e) => setPrivateKey(e.target.value)}
                className="font-mono pr-10"
                disabled={isLoading}
              />
              <button
                type="button"
                onClick={() => setShowPrivateKey(!showPrivateKey)}
                className="absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
              >
                {showPrivateKey ? (
                  <EyeOff className="h-4 w-4" />
                ) : (
                  <Eye className="h-4 w-4" />
                )}
              </button>
            </div>
          </div>

          {/* Label (optional) */}
          <div className="space-y-2">
            <Label htmlFor="label">
              Label <span className="text-muted-foreground">(optional)</span>
            </Label>
            <Input
              id="label"
              type="text"
              placeholder="e.g., Trading Wallet"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              disabled={isLoading}
            />
          </div>

          {/* Error Message */}
          {error && (
            <div className="flex items-center gap-2 p-3 rounded-lg bg-destructive/10 border border-destructive/20 text-destructive text-sm">
              <AlertTriangle className="h-4 w-4 shrink-0" />
              {error}
            </div>
          )}

          <DialogFooter className="gap-2 sm:gap-0">
            <Button
              type="button"
              variant="outline"
              onClick={handleClose}
              disabled={isLoading}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={isLoading}>
              {isLoading ? (
                <>
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  Connecting...
                </>
              ) : (
                'Connect Wallet'
              )}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// Button component that triggers the modal
interface ConnectWalletButtonProps {
  className?: string;
  variant?: 'default' | 'outline' | 'ghost';
  size?: 'default' | 'sm' | 'lg';
}

export function ConnectWalletButton({
  className,
  variant = 'default',
  size = 'default',
}: ConnectWalletButtonProps) {
  const [open, setOpen] = useState(false);

  return (
    <>
      <Button
        variant={variant}
        size={size}
        onClick={() => setOpen(true)}
        className={cn('gap-2', className)}
      >
        <Wallet className="h-4 w-4" />
        Connect Wallet
      </Button>
      <ConnectWalletModal open={open} onOpenChange={setOpen} />
    </>
  );
}
