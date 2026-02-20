"use client";

import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { cn, formatCurrency } from "@/lib/utils";
import type { Market } from "@/types/api";

interface MarketCardProps {
  market: Market;
  onSelect: (id: string) => void;
}

export function MarketCard({ market, onSelect }: MarketCardProps) {
  return (
    <Card
      className="cursor-pointer transition-all hover:border-primary hover:shadow-md"
      onClick={() => onSelect(market.id)}
    >
      <CardContent className="p-4 space-y-3">
        <div className="flex items-start justify-between gap-2">
          <p className="text-sm font-medium leading-tight line-clamp-2">
            {market.question}
          </p>
          <Badge variant="outline" className="shrink-0 text-xs">
            {market.category}
          </Badge>
        </div>

        <div className="flex items-center gap-3">
          <div className="flex-1">
            <p className="text-xs text-muted-foreground mb-1">Yes</p>
            <p className="text-lg font-bold tabular-nums text-profit">
              {(market.yes_price * 100).toFixed(0)}¢
            </p>
          </div>
          <div className="flex-1">
            <p className="text-xs text-muted-foreground mb-1">No</p>
            <p className="text-lg font-bold tabular-nums text-loss">
              {(market.no_price * 100).toFixed(0)}¢
            </p>
          </div>
        </div>

        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span>Vol 24h: {formatCurrency(market.volume_24h)}</span>
          <span>Liq: {formatCurrency(market.liquidity)}</span>
        </div>

        {!market.active && (
          <Badge variant="secondary" className="text-xs">
            Resolved
          </Badge>
        )}
      </CardContent>
    </Card>
  );
}
