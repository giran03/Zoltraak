import { useEffect, useRef } from "react";
import type { QueueItem } from "./snapshots";
import type { AddToast } from "./useToasts";

/** Group completions across queue pushes and video/music polling ticks. */
export function useDownloadNotifications(items: QueueItem[], addToast: AddToast) {
  const previous = useRef(new Map<string, string>());
  const pending = useRef(new Map<string, QueueItem>());
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const addRef = useRef(addToast); addRef.current = addToast;
  useEffect(() => () => { clearTimeout(timer.current); timer.current = undefined; pending.current.clear(); }, []);
  useEffect(() => {
    for (const item of items) {
      const before = previous.current.get(item.id);
      if (before && before !== item.state && (item.state === "finished" || item.state === "failed")) pending.current.set(item.id, item);
    }
    previous.current = new Map(items.map(item => [item.id, item.state]));
    if (!pending.current.size || timer.current !== undefined) return;
    timer.current = setTimeout(() => {
      timer.current = undefined;
      const changes = [...pending.current.values()]; pending.current.clear();
      for (const state of ["finished", "failed"] as const) {
        const batch = changes.filter(item => item.state === state);
        if (!batch.length) continue;
        addRef.current({ id: batch.length === 1 ? `${batch[0]!.id}-${state}` : `downloads-${state}-${Date.now()}`,
          type: state === "failed" ? "error" : "success",
          title: batch.length === 1 ? (state === "failed" ? "Download failed" : "Download completed") : `${batch.length} downloads ${state === "failed" ? "failed" : "completed"}`,
          message: batch.length === 1 ? `${batch[0]!.filename}${state === "failed" && batch[0]!.error ? `: ${batch[0]!.error}` : ""}` : batch.map(item => item.filename).join(", "),
          downloadIds: batch.map(item => item.id),
        });
      }
    }, 450);
  }, [items]);
}
