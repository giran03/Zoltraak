import { useCallback, useEffect, useRef, useState } from "react";
import { usePolling } from "./usePolling";
import {
  type Delivery,
  type DiscordSettings,
  type DiscordStatus,
  engine,
  hasEngine,
} from "./engine";

/**
 * The Discord delivery page's state.
 *
 * Polled rather than pushed, like `useVideo`: deliveries change on a human
 * timescale — one file every few minutes at most — and a push channel for
 * something the user is only looking at while the tab is open would be more
 * machinery than the problem needs.
 */
const POLL_MS = 1500;

export function useDiscord(active: boolean) {
  const [status, setStatus] = useState<DiscordStatus | null>(null);
  const [settings, setSettings] = useState<DiscordSettings | null>(null);
  const [deliveries, setDeliveries] = useState<Delivery[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pollError, setPollError] = useState<string | null>(null);
  /** Guards against a slow response landing after the tab has moved on. */
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    if (!hasEngine()) {
      setLoading(false);
      return;
    }
    try {
      const [nextStatus, nextSettings, nextDeliveries] = await Promise.all([
        engine.discordStatus(),
        engine.discordSettings(),
        engine.discordDeliveries(),
      ]);
      if (!alive.current) return;
      setStatus(nextStatus);
      setSettings(nextSettings);
      setDeliveries(nextDeliveries);
      setPollError(null);
    } catch (e) {
      if (alive.current) setPollError(e instanceof Error ? e.message : String(e));
    } finally {
      if (alive.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  usePolling(refresh, () => POLL_MS, active);

  /**
   * Save a change to the Discord settings.
   *
   * Applied to local state immediately so a toggle does not visibly snap back
   * while the round trip completes, then reconciled with what the engine
   * actually stored.
   */
  const commit = useCallback(
    async (patch: Partial<DiscordSettings>) => {
      if (!settings) return false;
      const next = { ...settings, ...patch };
      setSettings(next);
      try {
        const stored = await engine.updateDiscordSettings(next);
        if (alive.current) {
          setSettings(stored);
          setError(null);
        }
        return true;
      } catch (e) {
        if (alive.current) {
          setSettings(settings);
          setError(e instanceof Error ? e.message : String(e));
        }
        return false;
      }
    },
    [settings],
  );

  return { status, settings, deliveries, loading, error: error ?? pollError, setError, refresh, commit };
}

/** Sizes as the rest of the panel writes them, matching the engine's own. */
export function humanBytes(bytes: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  if (unit === 0) return `${bytes} B`;
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}
