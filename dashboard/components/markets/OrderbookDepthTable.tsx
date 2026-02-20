"use client";

import { cn } from "@/lib/utils";
import type { Orderbook } from "@/types/api";

interface OrderbookDepthTableProps {
  orderbook: Orderbook;
  outcome: "yes" | "no";
  onPriceClick?: (price: number) => void;
}

export function OrderbookDepthTable({
  orderbook,
  outcome,
  onPriceClick,
}: OrderbookDepthTableProps) {
  const bids = outcome === "yes" ? orderbook.yes_bids : orderbook.no_bids;
  const asks = outcome === "yes" ? orderbook.yes_asks : orderbook.no_asks;

  const maxBidQty = Math.max(...bids.map((b) => b.quantity), 1);
  const maxAskQty = Math.max(...asks.map((a) => a.quantity), 1);

  const spread =
    outcome === "yes" ? orderbook.spread.yes_spread : orderbook.spread.no_spread;

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <span className="uppercase font-medium">{outcome} Orderbook</span>
        <span>Spread: {(spread * 100).toFixed(1)}¢</span>
      </div>

      <div className="grid grid-cols-2 gap-2">
        {/* Bids */}
        <div>
          <div className="text-xs font-medium text-profit mb-1 px-1">Bids</div>
          <div className="space-y-0.5">
            {bids.slice(0, 8).map((level, i) => (
              <button
                key={i}
                onClick={() => onPriceClick?.(level.price)}
                className="relative w-full flex justify-between text-xs tabular-nums px-2 py-1 rounded hover:bg-profit/10 transition-colors"
              >
                <div
                  className="absolute inset-y-0 left-0 bg-profit/10 rounded"
                  style={{ width: `${(level.quantity / maxBidQty) * 100}%` }}
                />
                <span className="relative text-profit">
                  {(level.price * 100).toFixed(1)}¢
                </span>
                <span className="relative">{level.quantity.toFixed(0)}</span>
              </button>
            ))}
          </div>
        </div>

        {/* Asks */}
        <div>
          <div className="text-xs font-medium text-loss mb-1 px-1">Asks</div>
          <div className="space-y-0.5">
            {asks.slice(0, 8).map((level, i) => (
              <button
                key={i}
                onClick={() => onPriceClick?.(level.price)}
                className="relative w-full flex justify-between text-xs tabular-nums px-2 py-1 rounded hover:bg-loss/10 transition-colors"
              >
                <div
                  className="absolute inset-y-0 right-0 bg-loss/10 rounded"
                  style={{ width: `${(level.quantity / maxAskQty) * 100}%` }}
                />
                <span className="relative text-loss">
                  {(level.price * 100).toFixed(1)}¢
                </span>
                <span className="relative">{level.quantity.toFixed(0)}</span>
              </button>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
