import { useCallback, useRef, useState } from "react";
import { DEFAULT_ENGINE_SETTINGS, type EngineSettings } from "./engine";
import { useSettings } from "./useSettings";

export type SaveState = "idle" | "saving" | "saved" | "error";

/**
 * Settings that persist as you edit them.
 *
 * The previous design tracked a dirty flag, painted every changed field amber,
 * raised a warning banner, and made the app's navigation silently refuse to
 * move while anything was unsaved — a control that does nothing when clicked
 * reads as a frozen app. These are a handful of scalar fields written through
 * one engine call; committing them on blur removes the whole apparatus and the
 * class of bug that came with it.
 *
 * `commit` merges a patch into the current settings, so a caller only names
 * the field it changed and never has to reassemble the whole object.
 */
export function useAutosaveSettings() {
  const { settings, loading, error, clearError, save, refresh } = useSettings();
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const resetTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const commit = useCallback(
    async (patch: Partial<EngineSettings>) => {
      if (!settings) return false;

      // Nothing actually changed — a blur on an untouched field is the common
      // case and must not produce a write or a flash of "Saved".
      const unchanged = (Object.keys(patch) as (keyof EngineSettings)[]).every(
        (key) => JSON.stringify(settings[key]) === JSON.stringify(patch[key]),
      );
      if (unchanged) return true;

      clearTimeout(resetTimer.current);
      setSaveState("saving");
      const ok = await save({ ...settings, ...patch });
      setSaveState(ok ? "saved" : "error");
      if (ok) {
        resetTimer.current = setTimeout(() => setSaveState("idle"), 2000);
      }
      return ok;
    },
    [settings, save],
  );

  const resetToDefaults = useCallback(async () => {
    clearTimeout(resetTimer.current);
    setSaveState("saving");
    const ok = await save({ ...DEFAULT_ENGINE_SETTINGS });
    setSaveState(ok ? "saved" : "error");
    if (ok) {
      resetTimer.current = setTimeout(() => setSaveState("idle"), 2000);
    }
    return ok;
  }, [save]);

  return { settings, loading, error, clearError, refresh, commit, resetToDefaults, saveState };
}
