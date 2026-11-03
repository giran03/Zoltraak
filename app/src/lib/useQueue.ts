import { useCallback, useEffect, useRef, useState } from "react";
import { type DownloadSnapshot, type EngineSettings, engine, hasEngine, onQueue } from "./engine";
import { playCompletionSound } from "./completionSound";
import { applyRowDelta, reconcileRows } from "./polling";
import { usePolling } from "./usePolling";

export type QueueStatus = "connecting" | "live" | "detached";

/**
 * How long without a push before the heartbeat stops trusting it. The native
 * emitter sends a keepalive every 10 s while the window is shown.
 */
const PUSH_TIMEOUT = 15_000;

/**
 * Native queue state and system notifications. In-app notices are generated
 * from the merged queue/video/music snapshot by `useDownloadNotifications`.
 */
export function useQueue() {
  const [items, setItems] = useState<DownloadSnapshot[]>([]);
  const [status, setStatus] = useState<QueueStatus>("connecting");
  const [error, setError] = useState<string | null>(null);

  // The rows as last published, so an unchanged snapshot can be dropped before
  // it becomes a render.
  const itemsRef = useRef<DownloadSnapshot[]>([]);
  // When a push last arrived. The heartbeat reads this rather than the clock
  // alone: while the engine is pushing there is nothing for a poll to find.
  const lastPushRef = useRef(Date.now());
  const prevStatesRef = useRef<Map<string, { state: string; filename: string }>>(new Map());

  const settingsRef = useRef<EngineSettings | null>(null);

  useEffect(() => {
    if (typeof Notification !== "undefined" && Notification.permission === "default") {
      void Notification.requestPermission().catch(() => {});
    }

    if (hasEngine()) {
      void engine.settings().then((s) => {
        settingsRef.current = s;
      }).catch(() => {});
    }
  }, []);

  const sendOSNotification = (title: string, body: string) => {
    try {
      if (settingsRef.current && settingsRef.current.desktopNotifications === false) {
        return;
      }
      if (typeof Notification !== "undefined" && Notification.permission === "granted") {
        new Notification(title, { body });
      }
    } catch {
      // Ignore
    }
  };

  const handleNextItems = useCallback((incoming: DownloadSnapshot[]) => {
    setStatus("live");

    // Every snapshot arrives as fresh JSON off the IPC boundary, so rows that
    // did not change are still new objects. Reconciling them back to their
    // previous identities is what lets the register skip rows nothing
    // happened to — and an entirely idle tick stops here.
    const next = reconcileRows(itemsRef.current, incoming);
    if (next === itemsRef.current) return;
    itemsRef.current = next;

    const prevMap = prevStatesRef.current;
    const newMap = new Map<string, { state: string; filename: string }>();

    for (const item of next) {
      newMap.set(item.id, { state: item.state, filename: item.filename });
      const prev = prevMap.get(item.id);

      if (prev) {
        const wasInFlight = prev.state === "downloading" || prev.state === "waiting" || prev.state === "scheduled";
        if (wasInFlight && item.state === "finished") {
          sendOSNotification("Zoltraak — download finished", item.filename);

          if (!settingsRef.current || settingsRef.current.playSoundOnComplete !== false) {
            playCompletionSound();
          }

          // Loaded on the first finish rather than at startup.
          void import("./confetti").then((m) => m.triggerConfetti()).catch(() => {});
        } else if (wasInFlight && item.state === "failed") {
          sendOSNotification("Zoltraak — download failed", item.filename);
        }
      }
      // Deliberately no toast for a newly queued item. The user just added it
      // and the row appears immediately; twenty pasted URLs produced twenty
      // toasts, fifteen of which the visible cap silently dropped.
    }

    prevStatesRef.current = newMap;
    setItems(next);
  }, []);

  const refreshQueue = useCallback(async () => {
    if (!hasEngine()) return;
    try {
      const next = await engine.snapshot();
      handleNextItems(next);
    } catch {
      // Ignore background refresh error
    }
  }, [handleNextItems]);

  useEffect(() => {
    if (!hasEngine()) {
      setStatus("detached");
      return;
    }

    // Primary update path: Tauri push events keep the list in sync without polling.
    const unsubscribe = onQueue((push) => {
      lastPushRef.current = Date.now();
      if (Array.isArray(push)) {
        handleNextItems(push);
        return;
      }
      const next = applyRowDelta(itemsRef.current, push);
      // Drift (a delta that does not add up) is repaired from a full read.
      if (next) handleNextItems(next);
      else void refreshQueue();
    });

    let cancelled = false;
    engine
      .snapshot()
      .then((next) => {
        if (cancelled) return;
        handleNextItems(next);
      })
      .catch((e: Error) => {
        if (!cancelled) {
          setStatus("detached");
          setError(e.message);
        }
      });

    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, [handleNextItems, refreshQueue]);

  // Recovery only — the push channel is the real one. This exists for the gaps
  // in it: a dropped event, a machine coming back from suspend. While pushes
  // are arriving it does no reads at all.
  const heartbeat = useCallback(async () => {
    // Hidden, the emitter is deliberately silent; showing the window wakes
    // this and the emitter's full read catches up.
    if (document.visibilityState === "hidden") return;
    if (Date.now() - lastPushRef.current < PUSH_TIMEOUT) return;
    await refreshQueue();
  }, [refreshQueue]);
  usePolling(heartbeat, () => PUSH_TIMEOUT);

  const act = useCallback(
    async (fn: () => Promise<unknown>) => {
      setError(null);
      try {
        await fn();
        await refreshQueue();
        return true;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return false;
      }
    },
    [refreshQueue],
  );

  const add = useCallback(
    (url: string, directory?: string) => act(() => engine.add(url, directory)),
    [act],
  );

  const toggle = useCallback(
    (item: DownloadSnapshot) => {
      const running = item.state === "downloading" || item.state === "waiting";
      return act(() => (running ? engine.pause(item.id) : engine.resume(item.id)));
    },
    [act],
  );

  const remove = useCallback((id: string, deleteFile = false) => act(() => engine.remove(id, deleteFile)), [act]);
  const reveal = useCallback((id: string) => act(() => engine.reveal(id)), [act]);

  const pause = useCallback((id: string) => act(() => engine.pause(id)), [act]);
  const resume = useCallback((id: string) => act(() => engine.resume(id)), [act]);

  // Stable, so the register can skip renders that are only about other state.
  const clearError = useCallback(() => setError(null), []);

  return {
    items,
    status,
    error,
    clearError,
    add,
    toggle,
    pause,
    resume,
    remove,
    reveal,
    refresh: refreshQueue,
  };
}
