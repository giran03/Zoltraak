/** One poll at a time, including focus changes while a request is pending. */
export function startPolling(refresh: () => Promise<unknown>, delay: () => number) {
  let stopped = false;
  let running = false;
  let wakeRequested = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const wake = async () => {
    if (stopped) return;
    clearTimeout(timer);
    if (running) {
      wakeRequested = true;
      return;
    }
    running = true;
    try {
      await refresh();
    } finally {
      running = false;
      if (!stopped) {
        timer = setTimeout(() => void wake(), wakeRequested ? 0 : delay());
        wakeRequested = false;
      }
    }
  };
  void wake();
  return {
    wake: () => void wake(),
    stop: () => { stopped = true; clearTimeout(timer); },
  };
}

/** IPC contains JSON data. Retain unchanged rows and the array on idle ticks. */
function sameJson(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (!left || !right || typeof left !== "object" || typeof right !== "object") return false;
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left) && Array.isArray(right) && left.length === right.length
      && left.every((value, index) => sameJson(value, right[index]));
  }
  const a = left as Record<string, unknown>, b = right as Record<string, unknown>;
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length
    && keys.every(key => Object.prototype.hasOwnProperty.call(b, key) && sameJson(a[key], b[key]));
}

export function reconcileRows<T extends { id: string }>(previous: T[], incoming: T[]): T[] {
  const byId = new Map(previous.map((row) => [row.id, row]));
  const next = incoming.map((row) => {
    const old = byId.get(row.id);
    return old && sameJson(old, row) ? old : row;
  });
  return previous.length === next.length && next.every((row, i) => row === previous[i])
    ? previous : next;
}

/** A queue push that names only the rows that moved (see `queue_push.rs`). */
export interface RowDelta<T> {
  upserts: T[];
  removed: string[];
  order?: string[];
  count: number;
}

/**
 * Fold a delta into the rows last published. Returns the same array when
 * nothing changed, or `null` when the result disagrees with the sender's row
 * count — the caller should then fetch a full snapshot instead of trusting it.
 */
export function applyRowDelta<T extends { id: string }>(previous: T[], delta: RowDelta<T>): T[] | null {
  if (!delta.upserts.length && !delta.removed.length && !delta.order) {
    return previous.length === delta.count ? previous : null;
  }
  const byId = new Map(previous.map((row) => [row.id, row]));
  for (const id of delta.removed) byId.delete(id);
  const added: string[] = [];
  for (const row of delta.upserts) {
    const old = byId.get(row.id);
    if (!old) added.push(row.id);
    byId.set(row.id, old && sameJson(old, row) ? old : row);
  }
  const ids = delta.order ?? [...previous.map((row) => row.id).filter((id) => byId.has(id)), ...added];
  const next: T[] = [];
  for (const id of ids) {
    const row = byId.get(id);
    if (!row) return null;
    next.push(row);
  }
  if (next.length !== delta.count) return null;
  return next.length === previous.length && next.every((row, i) => row === previous[i]) ? previous : next;
}
