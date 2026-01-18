'use client';

import { useState, useEffect } from 'react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import { Loader2 } from 'lucide-react';
import type { Workspace, UpdateWorkspaceRequest } from '@/types/api';

interface QuickSettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workspace: Workspace | null;
  onSave: (updates: UpdateWorkspaceRequest) => Promise<void>;
  isSaving: boolean;
}

export function QuickSettingsDialog({
  open,
  onOpenChange,
  workspace,
  onSave,
  isSaving,
}: QuickSettingsDialogProps) {
  const [formData, setFormData] = useState({
    auto_optimize_enabled: false,
    optimization_interval_hours: 24,
    min_roi_30d: 5,
    min_sharpe: 1,
    min_win_rate: 50,
    min_trades_30d: 10,
  });

  useEffect(() => {
    if (workspace) {
      setFormData({
        auto_optimize_enabled: workspace.auto_optimize_enabled,
        optimization_interval_hours: workspace.optimization_interval_hours,
        min_roi_30d: workspace.min_roi_30d ?? 5,
        min_sharpe: workspace.min_sharpe ?? 1,
        min_win_rate: workspace.min_win_rate ?? 50,
        min_trades_30d: workspace.min_trades_30d ?? 10,
      });
    }
  }, [workspace]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    await onSave(formData);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[425px]">
        <DialogHeader>
          <DialogTitle>Optimizer Settings</DialogTitle>
          <DialogDescription>
            Configure auto-optimization criteria for wallet rotation.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit}>
          <div className="grid gap-4 py-4">
            {/* Enable/Disable Toggle */}
            <div className="flex items-center justify-between">
              <Label htmlFor="auto_optimize_enabled" className="text-sm font-medium">
                Auto-Optimizer
              </Label>
              <Switch
                id="auto_optimize_enabled"
                checked={formData.auto_optimize_enabled}
                onCheckedChange={(checked) =>
                  setFormData((prev) => ({
                    ...prev,
                    auto_optimize_enabled: checked,
                  }))
                }
              />
            </div>

            {/* Interval */}
            <div className="grid gap-2">
              <Label htmlFor="interval">Check Interval (hours)</Label>
              <Input
                id="interval"
                type="number"
                min={1}
                max={168}
                value={formData.optimization_interval_hours}
                onChange={(e) =>
                  setFormData((prev) => ({
                    ...prev,
                    optimization_interval_hours: parseInt(e.target.value) || 24,
                  }))
                }
              />
            </div>

            {/* Criteria */}
            <div className="space-y-3 pt-2 border-t">
              <p className="text-sm font-medium">Selection Criteria</p>

              <div className="grid grid-cols-2 gap-3">
                <div className="grid gap-2">
                  <Label htmlFor="min_roi" className="text-xs">
                    Min ROI (%)
                  </Label>
                  <Input
                    id="min_roi"
                    type="number"
                    step="0.1"
                    value={formData.min_roi_30d}
                    onChange={(e) =>
                      setFormData((prev) => ({
                        ...prev,
                        min_roi_30d: parseFloat(e.target.value) || 0,
                      }))
                    }
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="min_sharpe" className="text-xs">
                    Min Sharpe
                  </Label>
                  <Input
                    id="min_sharpe"
                    type="number"
                    step="0.1"
                    value={formData.min_sharpe}
                    onChange={(e) =>
                      setFormData((prev) => ({
                        ...prev,
                        min_sharpe: parseFloat(e.target.value) || 0,
                      }))
                    }
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="min_win_rate" className="text-xs">
                    Min Win Rate (%)
                  </Label>
                  <Input
                    id="min_win_rate"
                    type="number"
                    min={0}
                    max={100}
                    value={formData.min_win_rate}
                    onChange={(e) =>
                      setFormData((prev) => ({
                        ...prev,
                        min_win_rate: parseFloat(e.target.value) || 0,
                      }))
                    }
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="min_trades" className="text-xs">
                    Min Trades
                  </Label>
                  <Input
                    id="min_trades"
                    type="number"
                    min={0}
                    value={formData.min_trades_30d}
                    onChange={(e) =>
                      setFormData((prev) => ({
                        ...prev,
                        min_trades_30d: parseInt(e.target.value) || 0,
                      }))
                    }
                  />
                </div>
              </div>
            </div>
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={isSaving}>
              {isSaving ? (
                <>
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  Saving...
                </>
              ) : (
                'Save Changes'
              )}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
