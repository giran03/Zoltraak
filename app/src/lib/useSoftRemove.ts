import { useCallback, useEffect, useRef, useState } from "react";
import type { QueueItem } from "./snapshots";
import type { AddToast } from "./useToasts";

/** How long a removal can be taken back before it is committed. */
const GRACE_MS = 6000;

/**
 * Removal with an undo window.
 *
 * The engine has no soft delete — removal drops the row (and optionally the
 * downloaded file), and re-adding the URL
 * afterwards would start a *new* download from zero rather than restoring the
 * one that was thrown away. So the grace period lives here: the rows disappear
 * from the list immediately, and the engine is only told once the window
 * closes. Undo is then just forgetting we intended to.
 *
 * A quit inside the window leaves the rows in place. That is the safe
 * direction to fail: the user gets back something they asked to delete, rather
 * than losing something they meant to keep.
 */
export function useSoftRemove({
  addToast,
  commit,
}: {
  addToast: AddToast;
  /** Actually delete one item. Called once the undo window closes. */
  commit: (item: QueueItem, deleteFile: boolean) => Promise<unknown>;
}) {
  const [pending, setPending] = useState<Set<string>>(new Set());
  const timers = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());
  const commitRef = useRef(commit);
  commitRef.current = commit;

  // A pending removal that never got committed because the view unmounted is
  // still better left undone than half-done, so timers are only cleared.
  useEffect(() => {
    const map = timers.current;
    return () => {
      for (const timer of map.values()) clearTimeout(timer);
      map.clear();
    };
  }, []);

  const forget = useCallback((ids: string[]) => {
    for (const id of ids) {
      const timer = timers.current.get(id);
      if (timer) clearTimeout(timer);
      timers.current.delete(id);
    }
    setPending((prev) => {
      const next = new Set(prev);
      for (const id of ids) next.delete(id);
      return next;
    });
  }, []);

  const remove = useCallback(
    (items: QueueItem[], deleteFile = false) => {
      if (items.length === 0) return;

      setPending((prev) => {
        const next = new Set(prev);
        for (const item of items) next.add(item.id);
        return next;
      });

      const batchId = `remove-${Date.now()}`;
      for (const item of items) {
        const previous = timers.current.get(item.id);
        if (previous) clearTimeout(previous);
        const timer = setTimeout(() => {
          timers.current.delete(item.id);
          void commitRef.current(item, deleteFile).finally(() => {
            // Whether or not the engine accepted it, stop hiding the row: if
            // the delete failed the item is still there and pretending
            // otherwise would be a lie the next refresh contradicts anyway.
            setPending((prev) => {
              const next = new Set(prev);
              next.delete(item.id);
              return next;
            });
          });
        }, GRACE_MS);
        timers.current.set(item.id, timer);
      }

      const ids = items.map((item) => item.id);
      addToast({
        id: batchId,
        type: "info",
        title: deleteFile
          ? (items.length === 1 ? "Entry and file scheduled for deletion" : `${items.length} entries and files scheduled for deletion`)
          : (items.length === 1 ? "Removed from list" : `Removed ${items.length} from list`),
        message: deleteFile
          ? "The local file will be permanently deleted when Undo expires."
          : (items.length === 1 ? (items[0]?.filename ?? "") : "Files already downloaded are left on disk."),
        durationMs: GRACE_MS,
        action: { label: "Undo", onClick: () => forget(ids) },
      });
    },
    [addToast, forget],
  );

  return { pending, remove };
}
