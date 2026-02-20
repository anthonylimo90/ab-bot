"use client";

import { useState, useMemo, useCallback } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { useMarketsQuery } from "@/hooks/queries/useMarketsQuery";
import { MarketCard } from "@/components/markets/MarketCard";
import { MarketDetailSheet } from "@/components/markets/MarketDetailSheet";
import { BarChart2, Search } from "lucide-react";

export default function MarketsPage() {
  const [search, setSearch] = useState("");
  const [activeOnly, setActiveOnly] = useState(true);
  const [selectedMarketId, setSelectedMarketId] = useState<string | null>(null);

  const { data: markets = [], isLoading } = useMarketsQuery({
    active: activeOnly || undefined,
    limit: 100,
  });

  const filteredMarkets = useMemo(() => {
    if (!search.trim()) return markets;
    const lower = search.toLowerCase();
    return markets.filter(
      (m) =>
        m.question.toLowerCase().includes(lower) ||
        m.category.toLowerCase().includes(lower),
    );
  }, [markets, search]);

  return (
    <div className="space-y-5 sm:space-y-6 p-6">
      <div>
        <h1 className="flex items-center gap-2 text-2xl font-bold tracking-tight sm:text-3xl">
          <BarChart2 className="h-8 w-8" />
          Markets
        </h1>
        <p className="text-muted-foreground">
          Browse Polymarket prediction markets and orderbooks
        </p>
      </div>

      {/* Filters */}
      <div className="flex flex-col gap-4 sm:flex-row sm:items-center">
        <div className="relative flex-1">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
          <Input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search markets..."
            className="pl-10"
          />
        </div>
        <div className="flex items-center gap-2">
          <Switch
            id="active-only"
            checked={activeOnly}
            onCheckedChange={setActiveOnly}
          />
          <Label htmlFor="active-only" className="text-sm">
            Active only
          </Label>
        </div>
      </div>

      {/* Results count */}
      <div className="flex items-center gap-2">
        <Badge variant="secondary">{filteredMarkets.length} markets</Badge>
      </div>

      {/* Market Grid */}
      {isLoading ? (
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {Array.from({ length: 6 }).map((_, i) => (
            <Card key={i}>
              <CardContent className="p-6">
                <Skeleton className="h-4 w-full mb-3" />
                <Skeleton className="h-4 w-3/4 mb-4" />
                <div className="flex gap-4">
                  <Skeleton className="h-8 w-20" />
                  <Skeleton className="h-8 w-20" />
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      ) : filteredMarkets.length === 0 ? (
        <Card>
          <CardContent className="p-12 text-center">
            <BarChart2 className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
            <h3 className="text-lg font-medium mb-2">No markets found</h3>
            <p className="text-muted-foreground">
              Try adjusting your search or filters
            </p>
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {filteredMarkets.map((market) => (
            <MarketCard
              key={market.id}
              market={market}
              onSelect={setSelectedMarketId}
            />
          ))}
        </div>
      )}

      {/* Market Detail Sheet */}
      <MarketDetailSheet
        marketId={selectedMarketId}
        onClose={() => setSelectedMarketId(null)}
      />
    </div>
  );
}
