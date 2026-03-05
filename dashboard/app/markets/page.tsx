"use client";

import { useState, useMemo, useCallback } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { useMarketsQuery } from "@/hooks/queries/useMarketsQuery";
import { useMarketMetadataQuery } from "@/hooks/queries/useSignalsQuery";
import { MarketCard } from "@/components/markets/MarketCard";
import { MarketDetailSheet } from "@/components/markets/MarketDetailSheet";
import { InfoTooltip } from "@/components/shared/InfoTooltip";
import { PageIntro } from "@/components/shared/PageIntro";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { BarChart2, Search } from "lucide-react";

export default function MarketsPage() {
  const [search, setSearch] = useState("");
  const [activeOnly, setActiveOnly] = useState(true);
  const [category, setCategory] = useState<string>("all");
  const [selectedMarketId, setSelectedMarketId] = useState<string | null>(null);

  const { data: metadata = [] } = useMarketMetadataQuery({ limit: 250 });
  const categories = useMemo(() => {
    const cats = new Set<string>();
    metadata.forEach((m) => {
      if (m.category) cats.add(m.category);
    });
    return Array.from(cats).sort();
  }, [metadata]);

  const { data: markets = [], isLoading } = useMarketsQuery({
    active: activeOnly || undefined,
    category: category !== "all" ? category : undefined,
    limit: 100,
  });

  const filteredMarkets = useMemo(() => {
    if (!search.trim()) return markets;
    const lower = search.toLowerCase();
    return markets.filter(
      (m) =>
        m.question.toLowerCase().includes(lower) ||
        (m.category ?? "").toLowerCase().includes(lower),
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

      <PageIntro
        title="What you can do here"
        description="Browse live prediction markets, see the current Yes and No prices, and open a market to inspect its order book in more detail."
        bullets={[
          "A Yes price is what it currently costs to buy the 'this happens' side of a market.",
          "A No price is what it costs to buy the 'this does not happen' side.",
          "Open a market card to view price depth, spreads, and more details before acting."
        ]}
      />

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
        <Select value={category} onValueChange={setCategory}>
          <SelectTrigger className="w-[180px]">
            <SelectValue placeholder="All Categories" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All Categories</SelectItem>
            {categories.map((cat) => (
              <SelectItem key={cat} value={cat}>
                {cat}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <div className="flex items-center gap-2">
          <Switch
            id="active-only"
            checked={activeOnly}
            onCheckedChange={setActiveOnly}
          />
          <Label htmlFor="active-only" className="inline-flex items-center gap-1 text-sm">
            Active only
            <InfoTooltip content="When enabled, resolved or closed markets are hidden so you only see markets that can still move." />
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
