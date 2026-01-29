'use client';

import { useState } from 'react';
import { useAccount } from 'wagmi';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { useTradeSign } from '@/hooks/useTradeSign';
import { useToastStore } from '@/stores/toast-store';
import { Wallet, Loader2, Check, AlertCircle, ArrowUpRight, ArrowDownRight } from 'lucide-react';
import { cn } from '@/lib/utils';

interface WalletTradePanelProps {
  tokenId: string;
  marketQuestion?: string;
  currentYesPrice?: number;
  currentNoPrice?: number;
  negRisk?: boolean;
}

export function WalletTradePanel({
  tokenId,
  marketQuestion,
  currentYesPrice = 0.5,
  currentNoPrice = 0.5,
  negRisk = false,
}: WalletTradePanelProps) {
  const { address, isConnected } = useAccount();
  const toast = useToastStore();
  const { state, error, pendingOrder, prepareAndSign, reset } = useTradeSign();

  const [side, setSide] = useState<'BUY' | 'SELL'>('BUY');
  const [outcome, setOutcome] = useState<'yes' | 'no'>('yes');
  const [price, setPrice] = useState('');
  const [size, setSize] = useState('');

  const effectivePrice = outcome === 'yes' ? currentYesPrice : currentNoPrice;

  const handleTrade = async () => {
    const priceNum = parseFloat(price) || effectivePrice;
    const sizeNum = parseFloat(size);

    if (!sizeNum || sizeNum <= 0) {
      toast.error('Invalid size', 'Please enter a valid trade size');
      return;
    }

    if (priceNum <= 0 || priceNum >= 1) {
      toast.error('Invalid price', 'Price must be between 0 and 1');
      return;
    }

    const result = await prepareAndSign({
      tokenId,
      side,
      price: priceNum,
      size: sizeNum,
      negRisk,
    });

    if (result?.success) {
      toast.success('Order submitted', `Order ${result.order_id} placed successfully`);
      setSize('');
      reset();
    } else if (error) {
      toast.error('Order failed', error);
    }
  };

  if (!isConnected) {
    return (
      <Card>
        <CardContent className="pt-6">
          <div className="flex flex-col items-center justify-center py-8 text-center">
            <Wallet className="h-12 w-12 text-muted-foreground mb-4" />
            <p className="text-muted-foreground">
              Connect your wallet to trade with MetaMask
            </p>
          </div>
        </CardContent>
      </Card>
    );
  }

  const isProcessing = state === 'preparing' || state === 'awaiting_signature' || state === 'submitting';

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Wallet className="h-5 w-5" />
          Wallet Trade
        </CardTitle>
        <CardDescription>
          Sign orders with MetaMask - no private key storage
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {/* Market Info */}
        {marketQuestion && (
          <div className="rounded-lg bg-muted p-3">
            <p className="text-sm font-medium">{marketQuestion}</p>
            <div className="flex gap-4 mt-2 text-xs text-muted-foreground">
              <span>YES: ${currentYesPrice.toFixed(2)}</span>
              <span>NO: ${currentNoPrice.toFixed(2)}</span>
            </div>
          </div>
        )}

        {/* Side Selection */}
        <Tabs value={side} onValueChange={(v) => setSide(v as 'BUY' | 'SELL')}>
          <TabsList className="grid w-full grid-cols-2">
            <TabsTrigger value="BUY" className="data-[state=active]:bg-green-500/20 data-[state=active]:text-green-500">
              <ArrowUpRight className="h-4 w-4 mr-1" />
              Buy
            </TabsTrigger>
            <TabsTrigger value="SELL" className="data-[state=active]:bg-red-500/20 data-[state=active]:text-red-500">
              <ArrowDownRight className="h-4 w-4 mr-1" />
              Sell
            </TabsTrigger>
          </TabsList>
        </Tabs>

        {/* Outcome Selection */}
        <div className="grid grid-cols-2 gap-2">
          <Button
            variant={outcome === 'yes' ? 'default' : 'outline'}
            onClick={() => setOutcome('yes')}
            className={cn(outcome === 'yes' && 'bg-green-600 hover:bg-green-700')}
          >
            YES
          </Button>
          <Button
            variant={outcome === 'no' ? 'default' : 'outline'}
            onClick={() => setOutcome('no')}
            className={cn(outcome === 'no' && 'bg-red-600 hover:bg-red-700')}
          >
            NO
          </Button>
        </div>

        {/* Price Input */}
        <div className="space-y-2">
          <Label htmlFor="price">Limit Price</Label>
          <Input
            id="price"
            type="number"
            step="0.01"
            min="0.01"
            max="0.99"
            placeholder={effectivePrice.toFixed(2)}
            value={price}
            onChange={(e) => setPrice(e.target.value)}
          />
        </div>

        {/* Size Input */}
        <div className="space-y-2">
          <Label htmlFor="size">Size (USDC)</Label>
          <Input
            id="size"
            type="number"
            step="1"
            min="1"
            placeholder="100"
            value={size}
            onChange={(e) => setSize(e.target.value)}
          />
        </div>

        {/* Order Summary */}
        {pendingOrder && state === 'awaiting_signature' && (
          <div className="rounded-lg border border-yellow-500/50 bg-yellow-500/10 p-3 space-y-1">
            <p className="text-sm font-medium text-yellow-500">Waiting for signature...</p>
            <p className="text-xs text-muted-foreground">
              {pendingOrder.summary.side} {pendingOrder.summary.outcome} @ {pendingOrder.summary.price}
            </p>
            <p className="text-xs text-muted-foreground">
              Size: {pendingOrder.summary.size} → Payout: {pendingOrder.summary.potential_payout}
            </p>
          </div>
        )}

        {/* Error Display */}
        {error && (
          <div className="rounded-lg border border-red-500/50 bg-red-500/10 p-3">
            <p className="text-sm text-red-500 flex items-center gap-2">
              <AlertCircle className="h-4 w-4" />
              {error}
            </p>
          </div>
        )}

        {/* Success Display */}
        {state === 'success' && (
          <div className="rounded-lg border border-green-500/50 bg-green-500/10 p-3">
            <p className="text-sm text-green-500 flex items-center gap-2">
              <Check className="h-4 w-4" />
              Order submitted successfully!
            </p>
          </div>
        )}

        {/* Submit Button */}
        <Button
          onClick={handleTrade}
          disabled={isProcessing || !size}
          className={cn(
            'w-full',
            side === 'BUY' ? 'bg-green-600 hover:bg-green-700' : 'bg-red-600 hover:bg-red-700'
          )}
        >
          {isProcessing ? (
            <>
              <Loader2 className="h-4 w-4 mr-2 animate-spin" />
              {state === 'preparing' && 'Preparing order...'}
              {state === 'awaiting_signature' && 'Sign in MetaMask...'}
              {state === 'submitting' && 'Submitting...'}
            </>
          ) : (
            <>
              {side === 'BUY' ? 'Buy' : 'Sell'} {outcome.toUpperCase()}
            </>
          )}
        </Button>

        {/* Wallet Info */}
        <p className="text-xs text-center text-muted-foreground">
          Connected: {address?.slice(0, 6)}...{address?.slice(-4)}
        </p>
      </CardContent>
    </Card>
  );
}
