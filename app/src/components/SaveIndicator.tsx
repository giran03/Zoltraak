import type { SaveState } from "../lib/useAutosaveSettings";
import { Lamp, Legend } from "./Panel";

const LABEL: Record<SaveState, string> = {
  idle: "saves as you type",
  saving: "saving…",
  saved: "saved",
  error: "not saved",
};

const LAMP: Record<SaveState, string | null> = {
  idle: null,
  saving: "bg-caution",
  saved: "bg-clear",
  error: "bg-danger",
};

/**
 * Ambient save status for an autosaving form. Quiet by default — the point of
 * autosave is that the user stops thinking about saving, so this reassures
 * without demanding attention, and only raises its voice on failure.
 */
export function SaveIndicator({ state }: { state: SaveState }) {
  return (
    <span role="status" aria-live="polite" className="inline-flex items-center gap-2">
      <Lamp fill={LAMP[state]} size={7} />
      <Legend className={state === "error" ? "text-danger" : undefined}>{LABEL[state]}</Legend>
    </span>
  );
}
