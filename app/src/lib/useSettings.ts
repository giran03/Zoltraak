import { useCallback, useEffect, useState } from "react";
import { type EngineSettings, engine, hasEngine } from "./engine";

/**
 * Engine-wide settings: bandwidth cap, connection/folder defaults, and the
 * scheduler. Loaded once and refetched after each save — edits are rare
 * enough that a full round-trip is simpler than merging partial updates
 * locally.
 */
export function useSettings() {
  const [settings, setSettings] = useState<EngineSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!hasEngine()) {
      setLoading(false);
      return;
    }
    try {
      setSettings(await engine.settings());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    window.addEventListener("zoltraak:settings-saved", refresh);
    return () => window.removeEventListener("zoltraak:settings-saved", refresh);
  }, [refresh]);

  const save = useCallback(
    async (next: EngineSettings) => {
      setError(null);
      try {
        if (hasEngine()) {
          await engine.updateSettings(next);
        }
        setSettings(next);
        window.dispatchEvent(new Event("zoltraak:settings-saved"));
        return true;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return false;
      }
    },
    [],
  );

  return { settings, loading, error, clearError: () => setError(null), save, refresh };
}
