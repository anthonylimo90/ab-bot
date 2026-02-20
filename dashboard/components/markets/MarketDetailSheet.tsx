"use client";

import { useEffect } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
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

  if (!marketId) return null;

  return (
    <div className="fixed inset-0 z-50 bg-background/80 backdrop-blur-sm">
      <div className="fixed inset-y-0 right-0 w-full max-w-lg border-l bg-background shadow-lg overflow-y-auto">
        <div className="flex items-center justify-between p-4 border-b">
          <h2 className="text-lg font-semibold">Market Details</h2>
          <Button variant="ghost" size="icon" onClick={onClose}>
            <X className="h-4 w-4" />
          </Button>
        </div>

        <div className="p-4 space-y-4">
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
                <div className="flex flex-wrap gap-2 mt-2">
                  <Badge variant="outline">{market.category}</Badge>
                  <Badge variant={market.active ? "default" : "secondary"}>
                    {market.active ? "Active" : "Resolved"}
                  </Badge>
                </div>
              </div>

              {/* Prices */}
              <div className="grid grid-cols-2 gap-4">
                <Card>
                  <CardContent className="p-4 text-center">
                    <p className="text-xs text-muted-foreground mb-1">Yes Price</p>
                    <p className="text-2xl font-bold tabular-nums text-profit">
                      {(market.yes_price * 100).toFixed(1)}¢
                    </p>
                  </CardContent>
                </Card>
                <Card>
                  <CardContent className="p-4 text-center">
                    <p className="text-xs text-muted-foreground mb-1">No Price</p>
                    <p className="text-2xl font-bold tabular-nums text-loss">
                      {(market.no_price * 100).toFixed(1)}¢
                    </p>
                  </CardContent>
                </Card>
              </div>

              {/* Market stats */}
              <div className="grid grid-cols-2 gap-4 text-sm">
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
            <h3 className="text-sm font-medium mb-2">Orderbook</h3>
            {isOrderbookLoading ? (
              <div className="space-y-2">
                {Array.from({ length: 5 }).map((_, i) => (
                  <Skeleton key={i} className="h-6 w-full" />
                ))}
              </div>
            ) : orderbook ? (
              <div className="space-y-3">
                {/* Spread info */}
                <div className="flex gap-4 text-xs">
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
