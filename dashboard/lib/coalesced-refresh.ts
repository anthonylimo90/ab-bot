/** Bound refresh traffic, retain a trailing update, and never overlap refreshes. */
export function coalescedRefresh(refresh: () => Promise<unknown>, intervalMs = 1000) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let running = false;
  let dirty = false;
  let disposed = false;
  let lastStarted = -Infinity;

  function schedule() {
    if (disposed || running || timer !== undefined || !dirty) return;
    timer = setTimeout(async () => {
      timer = undefined;
      if (disposed) return;
      dirty = false;
      running = true;
      lastStarted = Date.now();
      try {
        await refresh();
      } catch {
        // Query state reports the error; another notification may retry.
      } finally {
        running = false;
        schedule();
      }
    }, Math.max(0, lastStarted + intervalMs - Date.now()));
  }

  return {
    notify() { dirty = true; schedule(); },
    dispose() { disposed = true; if (timer !== undefined) clearTimeout(timer); },
  };
}
