"use client";

import { Badge } from "@/components/ui/badge";
import { formatDynamicKey, formatDynamicConfigValue, formatTimeAgo } from "@/lib/utils";
import { cn } from "@/lib/utils";
import type { DynamicConfigHistoryEntry } from "@/types/api";
import { ArrowRight, Settings, RotateCcw, Snowflake, Eye, AlertTriangle } from "lucide-react";

interface TunerTimelineProps {
  history: DynamicConfigHistoryEntry[];
}

const ACTION_CONFIG: Record<string, { color: string; icon: React.ReactNode }> = {
  applied: { color: "bg-profit/10 text-profit", icon: <Settings className="h-3 w-3" /> },
  manual: { color: "bg-blue-500/10 text-blue-500", icon: <Settings className="h-3 w-3" /> },
  rollback: { color: "bg-yellow-500/10 text-yellow-600", icon: <RotateCcw className="h-3 w-3" /> },
  frozen: { color: "bg-purple-500/10 text-purple-500", icon: <Snowflake className="h-3 w-3" /> },
  watchdog: { color: "bg-orange-500/10 text-orange-500", icon: <AlertTriangle className="h-3 w-3" /> },
  shadow: { color: "bg-muted text-muted-foreground", icon: <Eye className="h-3 w-3" /> },
};

export function TunerTimeline({ history }: TunerTimelineProps) {
  if (history.length === 0) {
    return (
      <div className="py-8 text-center text-sm text-muted-foreground">
        No tuner history yet
      </div>
    );
  }

  return (
    <div className="max-h-80 space-y-2 overflow-y-auto">
      {history.map((entry) => {
        const config = ACTION_CONFIG[entry.action] || ACTION_CONFIG.applied;
        return (
          <div
            key={entry.id}
            className="flex items-start gap-3 rounded-lg border bg-background/50 p-3"
          >
            {/* Timeline dot */}
            <div className="mt-1 flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-muted">
              {config.icon}
            </div>

            <div className="min-w-0 flex-1 space-y-1">
              <div className="flex flex-wrap items-center gap-2">
                <Badge className={cn("text-xs", config.color)}>
                  {entry.action}
                </Badge>
                {entry.config_key && (
                  <span className="text-xs font-medium">
                    {formatDynamicKey(entry.config_key)}
                  </span>
                )}
                <span className="text-xs text-muted-foreground">
                  {formatTimeAgo(entry.created_at)}
                </span>
              </div>

              {/* Value change */}
              {entry.old_value != null && entry.new_value != null && (
                <div className="flex items-center gap-1.5 text-xs tabular-nums">
                  <span className="text-muted-foreground">
                    {formatDynamicConfigValue(entry.config_key, entry.old_value)}
                  </span>
                  <ArrowRight className="h-3 w-3 text-muted-foreground" />
                  <span className="font-medium">
                    {formatDynamicConfigValue(entry.config_key, entry.new_value)}
                  </span>
                </div>
              )}

              <p className="text-xs text-muted-foreground truncate">
                {entry.reason}
              </p>
            </div>
          </div>
        );
      })}
    </div>
  );
}
