import { useCallback, useEffect, useRef, useState } from "react";
import type { ToastItem } from "../components/Toast";

/** Default lifetime for every toast, including errors. */
const LIFETIME_MS = 6000;
const MAX_VISIBLE = 4;

export interface NotificationRecord {
  id: string;
  type: ToastItem["type"];
  title: string;
  message: string;
  timestamp: number;
  read: boolean;
  downloadIds?: string[];
}

const HISTORY_STORAGE_KEY = "zoltraak.notifications.v1";

function readStoredHistory(): NotificationRecord[] {
  try {
    const raw = localStorage.getItem(HISTORY_STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed)) {
        return parsed.filter((item): item is NotificationRecord => item && typeof item.id === "string" && ["success", "error", "info"].includes(item.type)
          && typeof item.title === "string" && typeof item.message === "string" && Number.isFinite(item.timestamp)).map(item => ({
            ...item, read: item.read === true,
            downloadIds: Array.isArray(item.downloadIds) ? [...new Set(item.downloadIds.filter((id): id is string => typeof id === "string"))] : undefined,
          }));
      }
    }
  } catch {
    // fallback
  }
  return [];
}

function writeStoredHistory(history: NotificationRecord[]) {
  try {
    localStorage.setItem(HISTORY_STORAGE_KEY, JSON.stringify(history));
  } catch {
    // storage quota or unavailable
  }
}

/**
 * The app's one toast queue and persistent notification log.
 */
export function useToasts(maxHistoryLimit: number = 25) {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const [history, setHistory] = useState<NotificationRecord[]>(readStoredHistory);
  const timers = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());
  const limitRef = useRef(maxHistoryLimit);

  useEffect(() => {
    const activeTimers = timers.current;
    return () => {
      for (const timer of activeTimers.values()) clearTimeout(timer);
      activeTimers.clear();
    };
  }, []);

  useEffect(() => {
    limitRef.current = maxHistoryLimit;
    setHistory((prev) => {
      if (prev.length > maxHistoryLimit) {
        const trimmed = prev.slice(0, maxHistoryLimit);
        writeStoredHistory(trimmed);
        return trimmed;
      }
      return prev;
    });
  }, [maxHistoryLimit]);

  const dismiss = useCallback((id: string) => {
    const timer = timers.current.get(id);
    if (timer) {
      clearTimeout(timer);
      timers.current.delete(id);
    }
    setToasts((prev) => prev.filter((toast) => toast.id !== id));
  }, []);

  const add = useCallback(
    (toast: ToastItem) => {
      const existing = timers.current.get(toast.id);
      if (existing) clearTimeout(existing);

      setToasts((prev) => [toast, ...prev.filter((t) => t.id !== toast.id)].slice(0, MAX_VISIBLE));

      // Append/update into persistent notification history capped by maxHistoryLimit
      setHistory((prev) => {
        const record: NotificationRecord = {
          id: toast.id,
          type: toast.type,
          title: toast.title,
          message: toast.message,
          timestamp: Date.now(),
          read: false,
          downloadIds: toast.downloadIds ? [...new Set(toast.downloadIds)] : undefined,
        };
        const next = [record, ...prev.filter((item) => item.id !== toast.id)].slice(
          0,
          limitRef.current,
        );
        writeStoredHistory(next);
        return next;
      });

      // Every popup expires; its message remains in notification history.
      // Invalid lifetimes must not accidentally bypass automatic dismissal.
      const durationMs = toast.durationMs ?? LIFETIME_MS;
      const timer = setTimeout(() => {
        timers.current.delete(toast.id);
        setToasts((prev) => prev.filter((t) => t.id !== toast.id));
      }, Number.isFinite(durationMs) && durationMs > 0 ? durationMs : LIFETIME_MS);
      timers.current.set(toast.id, timer);
    },
    [],
  );

  const markAllRead = useCallback(() => {
    setHistory((prev) => {
      const next = prev.map((item) => ({ ...item, read: true }));
      writeStoredHistory(next);
      return next;
    });
  }, []);

  const clearHistory = useCallback(() => {
    setHistory([]);
    writeStoredHistory([]);
  }, []);

  const removeNotification = useCallback((id: string) => {
    setHistory((prev) => {
      const next = prev.filter((item) => item.id !== id);
      writeStoredHistory(next);
      return next;
    });
  }, []);

  const unreadCount = history.filter((item) => !item.read).length;

  return {
    toasts,
    add,
    dismiss,
    history,
    unreadCount,
    markAllRead,
    clearHistory,
    removeNotification,
  };
}

export type AddToast = ReturnType<typeof useToasts>["add"];
