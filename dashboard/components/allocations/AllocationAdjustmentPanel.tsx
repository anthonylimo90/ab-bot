'use client';

import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table';
import { RefreshCw, CheckCircle, TrendingUp, TrendingDown, Eye } from 'lucide-react';
import { useToastStore } from '@/stores/toast-store';
import api from '@/lib/api';
import { RiskScoreDisplay } from './RiskScoreDisplay';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';

interface RiskComponents {
  sortino_normalized: number;
  consistency: number;
  roi_drawdown_ratio: number;
  win_rate: number;
  volatility: number;
}

interface AllocationPreview {
  address: string;
  current_allocation_pct: number | null;
  recommended_allocation_pct: number;
  change_pct: number;
  composite_score: number;
  components: RiskComponents;
}

interface RecalculateResponse {
  previews: AllocationPreview[];
  applied: boolean;
  wallet_count: number;
}

interface AllocationAdjustmentPanelProps {
  tier: 'active' | 'bench';
  className?: string;
}

export function AllocationAdjustmentPanel({ tier, className }: AllocationAdjustmentPanelProps) {
  const toast = useToastStore();
  const queryClient = useQueryClient();
  const [previews, setPreviews] = useState<AllocationPreview[]>([]);
  const [selectedWallet, setSelectedWallet] = useState<AllocationPreview | null>(null);

  const previewMutation = useMutation({
    mutationFn: async () => {
      return api.post<RecalculateResponse>('/api/v1/allocations/risk/recalculate', {
        tier,
        auto_apply: false,
      });
    },
    onSuccess: (data) => {
      setPreviews(data.previews);
      toast.success('Preview generated', `${data.wallet_count} wallets analyzed`);
    },
    onError: (error: Error) => {
      toast.error('Preview failed', error.message);
    },
  });

  const applyMutation = useMutation({
    mutationFn: async () => {
      return api.post<RecalculateResponse>('/api/v1/allocations/risk/recalculate', {
        tier,
        auto_apply: true,
      });
    },
    onSuccess: (data) => {
      queryClient.invalidateQueries({ queryKey: ['allocations'] });
      queryClient.invalidateQueries({ queryKey: ['wallets'] });
      setPreviews([]);
      toast.success('Allocations updated', `${data.wallet_count} wallets recalculated`);
    },
    onError: (error: Error) => {
      toast.error('Apply failed', error.message);
    },
  });

  const formatAddress = (address: string) => {
    return `${address.slice(0, 6)}...${address.slice(-4)}`;
  };

  const formatPercentage = (value: number) => {
    return `${value.toFixed(1)}%`;
  };

  const getChangeColor = (change: number) => {
    if (Math.abs(change) < 0.5) return 'text-muted-foreground';
    return change > 0 ? 'text-profit' : 'text-loss';
  };

  const getChangeIcon = (change: number) => {
    if (Math.abs(change) < 0.5) return null;
    return change > 0 ? <TrendingUp className="h-4 w-4" /> : <TrendingDown className="h-4 w-4" />;
  };

  return (
    <>
      <Card className={className}>
        <CardHeader>
          <div className="flex items-center justify-between">
            <div>
              <CardTitle>Risk-Based Allocation</CardTitle>
              <CardDescription>
                Dynamically adjust allocations based on composite risk scores
              </CardDescription>
            </div>
            <Button
              onClick={() => previewMutation.mutate()}
              disabled={previewMutation.isPending}
              variant="outline"
            >
              <RefreshCw className={`h-4 w-4 mr-2 ${previewMutation.isPending ? 'animate-spin' : ''}`} />
              Preview Changes
            </Button>
          </div>
        </CardHeader>

        {previews.length > 0 && (
          <CardContent className="space-y-4">
            <div className="rounded-lg border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Wallet</TableHead>
                    <TableHead className="text-right">Score</TableHead>
                    <TableHead className="text-right">Current</TableHead>
                    <TableHead className="text-right">Recommended</TableHead>
                    <TableHead className="text-right">Change</TableHead>
                    <TableHead className="text-right">Actions</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {previews.map((preview) => (
                    <TableRow key={preview.address}>
                      <TableCell className="font-mono text-sm">
                        {formatAddress(preview.address)}
                      </TableCell>
                      <TableCell className="text-right">
                        <Badge variant="outline">
                          {(preview.composite_score * 100).toFixed(0)}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-right text-muted-foreground">
                        {preview.current_allocation_pct !== null
                          ? formatPercentage(preview.current_allocation_pct)
                          : '—'}
                      </TableCell>
                      <TableCell className="text-right font-medium">
                        {formatPercentage(preview.recommended_allocation_pct)}
                      </TableCell>
                      <TableCell className={`text-right ${getChangeColor(preview.change_pct)}`}>
                        <div className="flex items-center justify-end gap-1">
                          {getChangeIcon(preview.change_pct)}
                          {formatPercentage(Math.abs(preview.change_pct))}
                        </div>
                      </TableCell>
                      <TableCell className="text-right">
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => setSelectedWallet(preview)}
                        >
                          <Eye className="h-4 w-4" />
                        </Button>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>

            <div className="flex items-center justify-between pt-4 border-t">
              <p className="text-sm text-muted-foreground">
                {previews.length} wallet{previews.length !== 1 ? 's' : ''} will be updated
              </p>
              <div className="flex gap-2">
                <Button
                  variant="outline"
                  onClick={() => setPreviews([])}
                  disabled={applyMutation.isPending}
                >
                  Cancel
                </Button>
                <Button
                  onClick={() => applyMutation.mutate()}
                  disabled={applyMutation.isPending}
                >
                  <CheckCircle className="h-4 w-4 mr-2" />
                  {applyMutation.isPending ? 'Applying...' : 'Apply Changes'}
                </Button>
              </div>
            </div>
          </CardContent>
        )}
      </Card>

      {/* Risk Score Detail Dialog */}
      <Dialog open={!!selectedWallet} onOpenChange={() => setSelectedWallet(null)}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>Risk Score Details</DialogTitle>
            <DialogDescription>
              Wallet: <span className="font-mono">{selectedWallet?.address}</span>
            </DialogDescription>
          </DialogHeader>

          {selectedWallet && (
            <div className="space-y-4">
              <RiskScoreDisplay
                compositeScore={selectedWallet.composite_score}
                components={selectedWallet.components}
              />

              <div className="grid grid-cols-2 gap-4 pt-4 border-t">
                <div>
                  <p className="text-sm text-muted-foreground mb-1">Current Allocation</p>
                  <p className="text-2xl font-bold">
                    {selectedWallet.current_allocation_pct !== null
                      ? formatPercentage(selectedWallet.current_allocation_pct)
                      : 'Not set'}
                  </p>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground mb-1">Recommended Allocation</p>
                  <p className="text-2xl font-bold text-primary">
                    {formatPercentage(selectedWallet.recommended_allocation_pct)}
                  </p>
                </div>
              </div>

              <div className="pt-2">
                <p className="text-sm text-muted-foreground mb-1">Change</p>
                <p className={`text-xl font-semibold ${getChangeColor(selectedWallet.change_pct)}`}>
                  {selectedWallet.change_pct > 0 ? '+' : ''}
                  {formatPercentage(selectedWallet.change_pct)}
                </p>
              </div>
            </div>
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}
