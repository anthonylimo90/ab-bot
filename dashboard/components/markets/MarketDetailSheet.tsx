"use client";

import { useEffect } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { InfoTooltip } from "@/components/shared/InfoTooltip";
import { useMarketQuery, useOrderbookQuery } from "@/hooks/queries/useMarketsQuery";
import { useWebSocket } from "@/hooks/useWebSocket";
import { formatCurrency } from "@/lib/utils";
import { X, TrendingUp, TrendingDown } from "lucide-react";
import type { WebSocketMessage, OrderbookUpdate } from "@/types/api";
import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { queryKeys } from "@/lib/queryClient";

interface MarketDetailSheetProps {
  marketId: string | null;
  onClose: () => void;
}

export function MarketDetailSheet({ marketId, onClose }: MarketDetailSheetProps) {
  const queryClient = useQueryClient();
  const { data: market, isLoading: isMarketLoading } = useMarketQuery(marketId);
  const { data: orderbook, isLoading: isOrderbookLoading } = useOrderbookQuery(marketId);

  const handleWsMessage = useCallback(
    (msg: WebSocketMessage) => {
      if (msg.type !== "Orderbook") return;
      const update = msg.data as OrderbookUpdate;
      if (update.market_id === marketId && marketId !== null) {
        queryClient.invalidateQueries({
          queryKey: queryKeys.markets.orderbook(marketId),
        });
      }
    },
    [marketId, queryClient],
  );

  useWebSocket({
    channel: "orderbook",
    onMessage: handleWsMessage,
    enabled: !!marketId,
  });

  useEffect(() => {
    if (!marketId) return;

    const originalOverflow = document.body.style.overflow;
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };

    document.body.style.overflow = "hidden";
    document.addEventListener("keydown", handleEscape);

    return () => {
      document.body.style.overflow = originalOverflow;
      document.removeEventListener("keydown", handleEscape);
    };
  }, [marketId, onClose]);

  if (!marketId) return null;

  return (
    <div
      className="fixed inset-0 z-50 bg-background/80 backdrop-blur-sm"
      role="dialog"
      aria-modal="true"
      aria-label="Market details"
      onClick={onClose}
    >
      <div
        className="fixed inset-y-0 right-0 w-full max-w-lg overflow-y-auto border-l bg-background shadow-lg"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="flex items-center justify-between p-4 border-b">
          <h2 className="text-lg font-semibold">Market Details</h2>
          <Button variant="ghost" size="icon" onClick={onClose} aria-label="Close market details">
            <X className="h-4 w-4" />
          </Button>
        </div>

        <div className="space-y-4 p-3 sm:p-4">
          {isMarketLoading ? (
            <div className="space-y-3">
              <Skeleton className="h-6 w-full" />
              <Skeleton className="h-4 w-3/4" />
              <Skeleton className="h-20 w-full" />
            </div>
          ) : market ? (
            <>
              <div>
                <p className="text-lg font-medium">{market.question}</p>
                {market.description && (
                  <p className="text-sm text-muted-foreground mt-1">
                    {market.description}
                  </p>
                )}
                <p className="mt-2 text-sm text-muted-foreground">
                  If you believe the event will happen, you would usually look at the Yes side. If you believe it will not happen, you would usually look at the No side.
                </p>
                <div className="flex flex-wrap gap-2 mt-2">
                  <Badge variant="outline">{market.category}</Badge>
                  <Badge variant={market.active ? "default" : "secondary"}>
                    {market.active ? "Active" : "Resolved"}
                  </Badge>
                </div>
              </div>

              {/* Prices */}
              <div className="grid gap-4 sm:grid-cols-2">
                <Card>
                  <CardContent className="p-4 text-center">
                    <p className="mb-1 inline-flex items-center gap-1 text-xs text-muted-foreground">
                      Yes Price
                      <InfoTooltip content="The current price for buying the 'Yes' outcome. A higher price means the market thinks this outcome is more likely." />
                    </p>
                    <p className="text-2xl font-bold tabular-nums text-profit">
                      {(market.yes_price * 100).toFixed(1)}¢
                    </p>
                  </CardContent>
                </Card>
                <Card>
                  <CardContent className="p-4 text-center">
                    <p className="mb-1 inline-flex items-center gap-1 text-xs text-muted-foreground">
                      No Price
                      <InfoTooltip content="The current price for buying the 'No' outcome. A higher price means the market thinks the event is less likely to happen." />
                    </p>
                    <p className="text-2xl font-bold tabular-nums text-loss">
                      {(market.no_price * 100).toFixed(1)}¢
                    </p>
                  </CardContent>
                </Card>
              </div>

              {/* Market stats */}
              <div className="grid gap-4 text-sm sm:grid-cols-2">
                <div>
                  <p className="text-muted-foreground">24h Volume</p>
                  <p className="font-medium">{formatCurrency(market.volume_24h)}</p>
                </div>
                <div>
                  <p className="text-muted-foreground">Liquidity</p>
                  <p className="font-medium">{formatCurrency(market.liquidity)}</p>
                </div>
                <div>
                  <p className="text-muted-foreground">End Date</p>
                  <p className="font-medium">
                    {new Date(market.end_date).toLocaleDateString()}
                  </p>
                </div>
              </div>
            </>
          ) : null}

          {/* Orderbook */}
          <div>
            <h3 className="mb-2 flex items-center gap-2 text-sm font-medium">
              <span>Orderbook</span>
              <InfoTooltip content="The order book shows the current buy and sell offers waiting in the market. Tighter spreads usually mean trading is easier." />
            </h3>
            {isOrderbookLoading ? (
              <div className="space-y-2">
                {Array.from({ length: 5 }).map((_, i) => (
                  <Skeleton key={i} className="h-6 w-full" />
                ))}
              </div>
            ) : orderbook ? (
              <div className="space-y-3">
                {/* Spread info */}
                <div className="flex flex-wrap gap-2 text-xs">
                  <Badge variant="outline">
                    Yes spread: {(orderbook.spread.yes_spread * 100).toFixed(1)}¢
                  </Badge>
                  <Badge variant="outline">
                    No spread: {(orderbook.spread.no_spread * 100).toFixed(1)}¢
                  </Badge>
                  {orderbook.spread.arb_spread != null && (
                    <Badge
                      variant={orderbook.spread.arb_spread < 0.98 ? "default" : "secondary"}
                    >
                      Arb: {(orderbook.spread.arb_spread * 100).toFixed(1)}¢
                    </Badge>
                  )}
                </div>

                {/* Yes side */}
                <div>
                  <p className="text-xs font-medium text-profit mb-1">Yes Bids</p>
                  <div className="space-y-0.5">
                    {orderbook.yes_bids.slice(0, 5).map((level, i) => (
                      <div
                        key={i}
                        className="flex justify-between text-xs tabular-nums px-2 py-1 bg-profit/5 rounded"
                      >
                        <span>{(level.price * 100).toFixed(1)}¢</span>
                        <span>{level.quantity.toFixed(0)}</span>
                      </div>
                    ))}
                  </div>
                </div>

                <div>
                  <p className="text-xs font-medium text-loss mb-1">Yes Asks</p>
                  <div className="space-y-0.5">
                    {orderbook.yes_asks.slice(0, 5).map((level, i) => (
                      <div
                        key={i}
                        className="flex justify-between text-xs tabular-nums px-2 py-1 bg-loss/5 rounded"
                      >
                        <span>{(level.price * 100).toFixed(1)}¢</span>
                        <span>{level.quantity.toFixed(0)}</span>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">No orderbook data</p>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
