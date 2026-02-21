"use client";

import { useState, useEffect } from "react";
import { formatTimeAgo, getTimeAgoRefreshInterval } from "@/lib/utils";

interface TimeAgoProps {
  date: Date | string;
  className?: string;
}

/**
 * Self-refreshing relative timestamp.
 *
 * Adapts its tick rate to the age of the timestamp:
 *  - Recent items (< 1 min) refresh every 10s so "12s ago" → "22s ago" is visible
 *  - Older items slow down to avoid unnecessary work
 *  - Items > 24h stop ticking entirely
 */
export function TimeAgo({ date, className }: TimeAgoProps) {
  const [, setTick] = useState(0);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;

    function schedule() {
      const interval = getTimeAgoRefreshInterval(date);
      if (interval === null) return; // nothing to update
      timer = setTimeout(() => {
        setTick((t) => t + 1);
        schedule(); // re-schedule with potentially new interval
      }, interval);
    }

    schedule();
    return () => {
      if (timer) clearTimeout(timer);
    };
  }, [date]);

  return <span className={className}>{formatTimeAgo(date)}</span>;
}
