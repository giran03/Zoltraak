import { useEffect, useRef, useState } from "react";
import { engine, hasEngine, type OutputStatus } from "./engine";
import type { QueueItem } from "./snapshots";

const STATUS_CHANGED_EVENT = "zoltraak:file-status-changed";

/** Notify all listeners that a download's file status has changed (e.g. located or redownloaded). */
export function notifyFileStatusChanged(id: string, status?: OutputStatus) {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent(STATUS_CHANGED_EVENT, { detail: { id, status } }));
  }
}

/**
 * Tracks disk availability for finished downloads across the table.
 * Refreshes when finished items change, on window focus, on tab visibility,
 * or when an explicit file status update event occurs.
 */
export function useFileStatuses(items: QueueItem[]): Record<string, OutputStatus> {
  const [statuses, setStatuses] = useState<Record<string, OutputStatus>>({});
  const itemsRef = useRef(items);
  itemsRef.current = items;
  const inFlightRef = useRef(false);

  // Group key of finished item IDs so we only re-run when finished items change
  const finishedKey = items
    .filter((item) => item.state === "finished")
    .map((item) => item.id)
    .sort()
    .join(",");

  useEffect(() => {
    if (!hasEngine()) return;

    let active = true;

    const checkFinished = async () => {
      if (document.hidden || inFlightRef.current) return;
      const finished = itemsRef.current.filter((item) => item.state === "finished");
      if (finished.length === 0) return;

      inFlightRef.current = true;
      try {
        const ids = finished.map((item) => item.id);
        const result = await engine.fileStatuses(ids);
        if (active) {
          setStatuses((prev) => ({ ...prev, ...result }));
        }
      } catch {
        // Ignore background query failures
      } finally {
        inFlightRef.current = false;
      }
    };

    void checkFinished();

    const handleFocus = () => {
      void checkFinished();
    };

    const handleVisibility = () => {
      if (!document.hidden) {
        void checkFinished();
      }
    };

    const handleStatusChanged = (e: Event) => {
      const detail = (e as CustomEvent<{ id?: string; status?: OutputStatus }>).detail;
      if (detail?.id && detail?.status) {
        setStatuses((prev) => ({ ...prev, [detail.id!]: detail.status! }));
      } else {
        void checkFinished();
      }
    };

    window.addEventListener("focus", handleFocus);
    document.addEventListener("visibilitychange", handleVisibility);
    window.addEventListener(STATUS_CHANGED_EVENT, handleStatusChanged);
    const interval = window.setInterval(checkFinished, 30_000);

    return () => {
      active = false;
      window.removeEventListener("focus", handleFocus);
      document.removeEventListener("visibilitychange", handleVisibility);
      window.removeEventListener(STATUS_CHANGED_EVENT, handleStatusChanged);
      window.clearInterval(interval);
    };
  }, [finishedKey]);

  return statuses;
}
