import { useSyncExternalStore } from "react";
import { defaultShortcuts, readShortcuts, SHORTCUTS_KEY, type ShortcutSettings } from "./shortcuts.ts";
let current: ShortcutSettings;
try { current = readShortcuts(localStorage.getItem(SHORTCUTS_KEY)); } catch { current = defaultShortcuts(); }
const listeners = new Set<() => void>();
function publish(value: ShortcutSettings) { current = value; listeners.forEach(listener => listener()); }
if (typeof window !== "undefined") window.addEventListener("storage", event => {
  if (event.key === SHORTCUTS_KEY || event.key === null) {
    try { publish(readShortcuts(localStorage.getItem(SHORTCUTS_KEY))); } catch { /* Retain current settings. */ }
  }
});
export function saveShortcuts(value: ShortcutSettings) {
  try { localStorage.setItem(SHORTCUTS_KEY, JSON.stringify(value)); } catch {
    return false;
  }
  publish(value);
  return true;
}
export function useShortcuts() {
  return useSyncExternalStore(listener => { listeners.add(listener); return () => listeners.delete(listener); }, () => current);
}
