"use client";

import { useState, useCallback } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { useTradeSign } from "@/hooks/useTradeSign";
import { useOrderbookQuery } from "@/hooks/queries/useMarketsQuery";
import { formatCurrency } from "@/lib/utils";
import { Loader2, CheckCircle, AlertCircle, Wallet } from "lucide-react";
import type { OrderSide } from "@/types/api";

interface OrderFormProps {
  marketId: string;
  tokenId: string;
  outcome: "yes" | "no";
  currentPrice: number;
  onSuccess?: (orderId: string) => void;
  onClose?: () => void;
}

export function OrderForm({
  marketId,
  tokenId,
  outcome,
  currentPrice,
  onSuccess,
  onClose,
}: OrderFormProps) {
  const [side, setSide] = useState<"BUY" | "SELL">("BUY");
  const [price, setPrice] = useState(currentPrice);
  const [size, setSize] = useState(10);

  const {
    state,
    error,
    pendingOrder,
    submittedOrder,
    prepareAndSign,
    reset,
    isConnected,
    address,
  } = useTradeSign();

  const totalCost = price * size;
  const potentialPayout = side === "BUY" ? size - totalCost : totalCost;

  const handleSubmit = useCallback(async () => {
    const result = await prepareAndSign({
      tokenId,
      side,
      price,
      size,
    });
    if (result?.order_id) {
      onSuccess?.(result.order_id);
    }
  }, [prepareAndSign, tokenId, side, price, size, onSuccess]);

  if (!isConnected) {
    return (
      <Card>
        <CardContent className="p-6 text-center">
          <Wallet className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
          <h3 className="text-lg font-medium mb-2">Connect Your Wallet</h3>
          <p className="text-sm text-muted-foreground">
            Connect a wallet with MetaMask to place manual orders
          </p>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-sm font-medium">Place Order</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        {/* Side Toggle */}
        <div className="grid grid-cols-2 gap-2">
          <Button
            variant={side === "BUY" ? "default" : "outline"}
            size="sm"
            onClick={() => setSide("BUY")}
            className={side === "BUY" ? "bg-profit hover:bg-profit/90" : ""}
          >
            Buy
          </Button>
          <Button
            variant={side === "SELL" ? "default" : "outline"}
            size="sm"
            onClick={() => setSide("SELL")}
            className={side === "SELL" ? "bg-loss hover:bg-loss/90" : ""}
          >
            Sell
          </Button>
        </div>

        {/* Price */}
        <div className="space-y-1.5">
          <Label htmlFor="order-price">Price (¢)</Label>
          <Input
            id="order-price"
            type="number"
            min={1}
            max={99}
            step={1}
            value={Math.round(price * 100)}
            onChange={(e) => setPrice(Number(e.target.value) / 100)}
          />
        </div>

        {/* Size */}
        <div className="space-y-1.5">
          <Label htmlFor="order-size">Size (shares)</Label>
          <Input
            id="order-size"
            type="number"
            min={1}
            step={1}
            value={size}
            onChange={(e) => setSize(Number(e.target.value))}
          />
        </div>

        {/* Summary */}
        <div className="rounded-lg border bg-muted/30 p-3 space-y-1 text-sm">
          <div className="flex justify-between">
            <span className="text-muted-foreground">Total Cost</span>
            <span className="font-medium tabular-nums">{formatCurrency(totalCost)}</span>
          </div>
          <div className="flex justify-between">
            <span className="text-muted-foreground">Potential Payout</span>
            <span className="font-medium tabular-nums">{formatCurrency(potentialPayout)}</span>
          </div>
          <div className="flex justify-between">
            <span className="text-muted-foreground">Outcome</span>
            <Badge variant="outline" className="text-xs uppercase">
              {outcome}
            </Badge>
          </div>
        </div>

        {/* Error */}
        {error && (
          <div className="flex items-center gap-2 text-sm text-loss">
            <AlertCircle className="h-4 w-4" />
            {error}
          </div>
        )}

        {/* Success */}
        {state === "success" && submittedOrder && (
          <div className="flex items-center gap-2 text-sm text-profit">
            <CheckCircle className="h-4 w-4" />
            Order placed successfully
          </div>
        )}

        {/* Submit */}
        <Button
          className="w-full"
          onClick={state === "success" || state === "error" ? reset : handleSubmit}
          disabled={state === "preparing" || state === "awaiting_signature" || state === "submitting"}
        >
          {state === "preparing" && (
            <>
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              Preparing...
            </>
          )}
          {state === "awaiting_signature" && (
            <>
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              Sign in Wallet...
            </>
          )}
          {state === "submitting" && (
            <>
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              Submitting...
            </>
          )}
          {state === "idle" && `${side} ${outcome.toUpperCase()}`}
          {state === "success" && "Place Another Order"}
          {state === "error" && "Try Again"}
        </Button>
      </CardContent>
    </Card>
  );
}
