"use client";

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { ArrowLeft, ArrowRight, Shield, Target, Zap } from "lucide-react";
import { RISK_PRESETS, type RiskPreset } from "@/lib/riskPresets";
import { cn } from "@/lib/utils";

interface RiskToleranceStepProps {
  onNext: (preset: RiskPreset) => void;
  onBack: () => void;
}

const PRESET_ICONS: Record<RiskPreset, React.ReactNode> = {
  conservative: <Shield className="h-10 w-10 text-blue-500" />,
  balanced: <Target className="h-10 w-10 text-primary" />,
  aggressive: <Zap className="h-10 w-10 text-yellow-500" />,
};

export function RiskToleranceStep({ onNext, onBack }: RiskToleranceStepProps) {
  const [selected, setSelected] = useState<RiskPreset>("balanced");

  return (
    <div className="space-y-6">
      <div className="text-center space-y-2">
        <h2 className="text-2xl font-bold">Choose Your Risk Tolerance</h2>
        <p className="text-muted-foreground">
          This controls how aggressively the system discovers and rotates wallets
        </p>
      </div>

      <div className="grid gap-4 md:grid-cols-3">
        {(Object.keys(RISK_PRESETS) as RiskPreset[]).map((preset) => {
          const config = RISK_PRESETS[preset];
          const isSelected = selected === preset;
          return (
            <Card
              key={preset}
              className={cn(
                "cursor-pointer transition-all hover:border-primary hover:shadow-md",
                isSelected && "border-primary ring-2 ring-primary/20",
              )}
              onClick={() => setSelected(preset)}
            >
              <CardHeader>
                {PRESET_ICONS[preset]}
                <CardTitle className="mt-2">{config.label}</CardTitle>
                <CardDescription>{config.description}</CardDescription>
              </CardHeader>
              <CardContent>
                <ul className="text-xs space-y-1 text-muted-foreground">
                  <li>Min ROI: {config.settings.min_roi_30d}%</li>
                  <li>Min Sharpe: {config.settings.min_sharpe}</li>
                  <li>Min Win Rate: {config.settings.min_win_rate}%</li>
                  <li>Run every {config.settings.optimization_interval_hours}h</li>
                </ul>
              </CardContent>
            </Card>
          );
        })}
      </div>

      <div className="flex justify-between pt-4">
        <Button variant="outline" onClick={onBack}>
          <ArrowLeft className="mr-2 h-4 w-4" />
          Back
        </Button>
        <Button onClick={() => onNext(selected)}>
          Continue
          <ArrowRight className="ml-2 h-4 w-4" />
        </Button>
      </div>
    </div>
  );
}
