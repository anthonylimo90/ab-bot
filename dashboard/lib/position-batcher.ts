import type { PositionUpdateType } from "@/types/api";

export interface PositionUpdateAccumulator {
  updates: Map<string, { price: number; pnl: number; quantity: number }>;
  opened: string[];
  closed: string[];
}

function emptyAccumulator(): PositionUpdateAccumulator {
  return { updates: new Map(), opened: [], closed: [] };
}

/**
 * Collects position updates and delivers them to `onUpdate` at most once per
 * `debounceMs`. Framework-free so the timer behaviour can be tested directly.
 */
export function createPositionBatcher(
  onUpdate: (accumulator: PositionUpdateAccumulator) => void,
  debounceMs = 200,
) {
  let accumulator = emptyAccumulator();
  let timer: ReturnType<typeof setTimeout> | null = null;

  function flush() {
    // Always clear the timer handle, even when there is nothing to deliver.
    // Leaving a stale handle behind makes addUpdate believe a flush is still
    // scheduled, so no later update would ever be delivered.
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    const pending = accumulator;
    if (
      pending.updates.size === 0 &&
      pending.opened.length === 0 &&
      pending.closed.length === 0
    ) {
      return;
    }
    accumulator = emptyAccumulator();
    onUpdate(pending);
  }

  function addUpdate(
    positionId: string,
    updateType: PositionUpdateType,
    data?: { price?: number; pnl?: number; quantity?: number },
  ) {
    switch (updateType) {
      case "opened":
        accumulator.opened.push(positionId);
        break;
      case "closed":
        accumulator.closed.push(positionId);
        // A close supersedes any price update still waiting in this window.
        accumulator.updates.delete(positionId);
        break;
      case "updated":
      case "price_changed":
        if (data) {
          const existing = accumulator.updates.get(positionId);
          accumulator.updates.set(positionId, {
            price: data.price ?? existing?.price ?? 0,
            pnl: data.pnl ?? existing?.pnl ?? 0,
            quantity: data.quantity ?? existing?.quantity ?? 0,
          });
        }
        break;
    }

    if (timer === null) {
      timer = setTimeout(flush, debounceMs);
    }
  }

  /** Drop pending work and the timer (unmount). The batcher stays reusable. */
  function dispose() {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    accumulator = emptyAccumulator();
  }

  return { addUpdate, flush, dispose };
}
