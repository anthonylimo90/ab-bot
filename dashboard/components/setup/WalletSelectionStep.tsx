'use client';

import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Progress } from '@/components/ui/progress';
import {
  ArrowLeft,
  ArrowRight,
  Plus,
  Minus,
  Star,
  TrendingUp,
  Loader2,
  Check,
  Eye,
  X,
} from 'lucide-react';
import { useToastStore } from '@/stores/toast-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import api from '@/lib/api';
import { ratioOrPercentToPercent } from '@/lib/utils';
import type { DiscoveredWallet } from '@/types/api';

interface WalletSelectionStepProps {
  onComplete: (walletCount: number) => void;
  onBack: () => void;
}

export function WalletSelectionStep({ onComplete, onBack }: WalletSelectionStepProps) {
  const queryClient = useQueryClient();
  const toast = useToastStore();
  const { currentWorkspace } = useWorkspaceStore();

  // Fetch discovered wallets
  const { data: discoveredWallets, isLoading: isLoadingWallets } = useQuery({
    queryKey: ['discover', 'wallets', currentWorkspace?.id],
    queryFn: () => api.discoverWallets({ sort_by: 'roi', period: '30d', limit: 50 }),
    enabled: !!currentWorkspace?.id,
  });

  // Fetch current allocations
  const { data: allocations, isLoading: isLoadingAllocations } = useQuery({
    queryKey: ['allocations', 'workspace', currentWorkspace?.id],
    queryFn: () => api.listAllocations(),
    enabled: !!currentWorkspace?.id,
  });

  const addMutation = useMutation({
    mutationFn: (address: string) => api.addAllocation(address, { tier: 'bench' }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['allocations', 'workspace', currentWorkspace?.id] });
      toast.success('Wallet added', 'Added to watching list');
    },
    onError: (error: Error) => {
      toast.error('Failed to add wallet', error.message);
    },
  });

  const promoteMutation = useMutation({
    mutationFn: (address: string) => api.promoteAllocation(address),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['allocations', 'workspace', currentWorkspace?.id] });
      toast.success('Wallet promoted', 'Now copying trades from this wallet');
    },
    onError: (error: Error) => {
      toast.error('Failed to promote wallet', error.message);
    },
  });

  const demoteMutation = useMutation({
    mutationFn: (address: string) => api.demoteAllocation(address),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['allocations', 'workspace', currentWorkspace?.id] });
      toast.success('Wallet demoted', 'Moved back to watching list');
    },
    onError: (error: Error) => {
      toast.error('Failed to demote wallet', error.message);
    },
  });

  const removeMutation = useMutation({
    mutationFn: (address: string) => api.removeAllocation(address),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['allocations', 'workspace', currentWorkspace?.id] });
      toast.success('Wallet removed', 'Removed from your list');
    },
    onError: (error: Error) => {
      toast.error('Failed to remove wallet', error.message);
    },
  });

  const activeCount = allocations?.filter((a) => a.tier === 'active').length ?? 0;
  const benchCount = allocations?.filter((a) => a.tier === 'bench').length ?? 0;
  const canAddActive = activeCount < 5;

  const isInRoster = (address: string) => {
    return allocations?.some((a) => a.wallet_address === address);
  };

  const getAllocation = (address: string) => {
    return allocations?.find((a) => a.wallet_address === address);
  };

  const formatPercent = (value: number) => {
    return `${ratioOrPercentToPercent(value).toFixed(1)}%`;
  };

  const formatAddress = (address: string) => {
    return `${address.slice(0, 6)}...${address.slice(-4)}`;
  };

  const isLoading = isLoadingWallets || isLoadingAllocations;

  return (
    <div className="space-y-6">
      <div className="text-center space-y-2">
        <h2 className="text-2xl font-bold">Build Your Active Portfolio</h2>
        <p className="text-muted-foreground">
          Select wallets to copy trades from. You can have up to 5 active wallets.
        </p>
      </div>

      {/* Active Wallets Progress */}
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium flex items-center gap-2">
            <Star className="h-4 w-4 text-yellow-500" />
            Active Wallets (copying trades)
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-2">
          <Progress value={(activeCount / 5) * 100} className="h-2" />
          <div className="flex justify-between text-sm">
            <span className="text-muted-foreground">{activeCount}/5 active slots</span>
            <span className="text-muted-foreground flex items-center gap-1">
              <Eye className="h-3 w-3" />
              {benchCount} watching
            </span>
          </div>
        </CardContent>
      </Card>

      {isLoading ? (
        <div className="flex items-center justify-center py-12">
          <Loader2 className="h-8 w-8 animate-spin text-primary" />
        </div>
      ) : (
        <>
          {/* Active Wallets */}
          {(allocations?.filter((a) => a.tier === 'active').length ?? 0) > 0 && (
            <div className="space-y-3">
              <h3 className="font-medium flex items-center gap-2">
                <Star className="h-4 w-4 text-yellow-500" />
                Active (Copying Trades)
              </h3>
              <div className="space-y-2">
                {allocations
                  ?.filter((a) => a.tier === 'active')
                  .map((allocation) => (
                    <div
                      key={allocation.wallet_address}
                      className="flex items-center justify-between p-3 rounded-lg border bg-green-500/5 border-green-500/20"
                    >
                      <div className="flex items-center gap-3">
                        <Star className="h-4 w-4 text-yellow-500 shrink-0" />
                        <div>
                          <p className="font-mono text-sm">
                            {formatAddress(allocation.wallet_address)}
                          </p>
                          <p className="text-xs text-muted-foreground">
                            Allocation: {allocation.allocation_pct}%
                          </p>
                        </div>
                      </div>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => demoteMutation.mutate(allocation.wallet_address)}
                        disabled={demoteMutation.isPending}
                      >
                        <Minus className="h-4 w-4 mr-1" />
                        Stop Copying
                      </Button>
                    </div>
                  ))}
              </div>
            </div>
          )}

          {/* Watching Wallets */}
          {(allocations?.filter((a) => a.tier === 'bench').length ?? 0) > 0 && (
            <div className="space-y-3">
              <h3 className="font-medium flex items-center gap-2">
                <Eye className="h-4 w-4 text-muted-foreground" />
                Watching (Not Copying Yet)
              </h3>
              <div className="space-y-2">
                {allocations
                  ?.filter((a) => a.tier === 'bench')
                  .map((allocation) => (
                    <div
                      key={allocation.wallet_address}
                      className="flex items-center justify-between p-3 rounded-lg border"
                    >
                      <div className="flex items-center gap-3">
                        <Eye className="h-4 w-4 text-muted-foreground shrink-0" />
                        <span className="font-mono text-sm">
                          {formatAddress(allocation.wallet_address)}
                        </span>
                      </div>
                      <div className="flex gap-2">
                        <Button
                          variant="default"
                          size="sm"
                          onClick={() => promoteMutation.mutate(allocation.wallet_address)}
                          disabled={!canAddActive || promoteMutation.isPending}
                        >
                          <Plus className="h-4 w-4 mr-1" />
                          Start Copying
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => removeMutation.mutate(allocation.wallet_address)}
                          disabled={removeMutation.isPending}
                        >
                          <X className="h-4 w-4" />
                        </Button>
                      </div>
                    </div>
                  ))}
              </div>
            </div>
          )}

          {/* Discovered Wallets */}
          <div className="space-y-3">
            <h3 className="font-medium flex items-center gap-2">
              <TrendingUp className="h-4 w-4 text-green-500" />
              Top Performing Wallets
            </h3>
            <p className="text-sm text-muted-foreground">
              These wallets have shown strong performance. Add them to start watching or copying.
            </p>
            <div className="space-y-2 max-h-[400px] overflow-y-auto">
              {discoveredWallets
                ?.filter((w) => !isInRoster(w.address))
                .slice(0, 20)
                .map((wallet) => (
                  <div
                    key={wallet.address}
                    className="flex items-center justify-between p-3 rounded-lg border hover:bg-muted/50 transition-colors"
                  >
                    <div className="space-y-1">
                      <div className="flex items-center gap-2">
                        <span className="font-mono text-sm">{formatAddress(wallet.address)}</span>
                        <Badge variant="outline" className="text-xs">
                          #{wallet.rank}
                        </Badge>
                      </div>
                      <div className="flex gap-3 text-xs text-muted-foreground">
                        <span className="text-green-600 font-medium">ROI: {formatPercent(wallet.roi_30d)}</span>
                        <span>Win: {formatPercent(wallet.win_rate)}</span>
                        <span>Trades: {wallet.total_trades}</span>
                      </div>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => addMutation.mutate(wallet.address)}
                      disabled={addMutation.isPending}
                    >
                      <Plus className="mr-1 h-4 w-4" />
                      Add to Watch
                    </Button>
                  </div>
                ))}
              {discoveredWallets?.filter((w) => !isInRoster(w.address)).length === 0 && (
                <p className="text-sm text-muted-foreground text-center py-4">
                  All available wallets have been added to your list.
                </p>
              )}
            </div>
          </div>
        </>
      )}

      {/* Navigation */}
      <div className="flex justify-between pt-4">
        <Button variant="outline" onClick={onBack}>
          <ArrowLeft className="mr-2 h-4 w-4" />
          Back
        </Button>
        <Button onClick={() => onComplete(activeCount)} disabled={activeCount < 1}>
          <Check className="mr-2 h-4 w-4" />
          Complete Setup ({activeCount} wallet{activeCount !== 1 ? 's' : ''})
        </Button>
      </div>
    </div>
  );
}
