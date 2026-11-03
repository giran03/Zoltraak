import { useEffect, useState, useSyncExternalStore } from "react";
import { DEFAULT_DARK_COLORS, resolvedIconPalette, useAppearance } from "./useAppearance";

export function useSystemDark() {
  const [dark, setDark] = useState(() => matchMedia("(prefers-color-scheme: dark)").matches);
  useEffect(() => {
    const media = matchMedia("(prefers-color-scheme: dark)");
    const change = () => setDark(media.matches);
    media.addEventListener("change", change);
    return () => media.removeEventListener("change", change);
  }, []);
  return dark;
}
type IconStatus = { busy: boolean; message: string; error: boolean };
let status: IconStatus = { busy: false, message: "", error: false };
const listeners = new Set<() => void>();
const publish = (next: IconStatus) => { status = next; listeners.forEach(fn => fn()); };
export function useIconStatus() {
  return useSyncExternalStore(fn => { listeners.add(fn); return () => { listeners.delete(fn); }; }, () => status);
}
let sequence = 0;
let pending: Promise<void> = Promise.resolve();
export function refreshAppIcon(foreground: string, force = true) {
  const request = ++sequence;
  publish({ busy: true, message: "Updating app icons…", error: false });
  pending = pending.catch(() => {}).then(async () => {
    if (request !== sequence) return;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const result = await invoke<{ shortcutsUpdated: number; warning: string | null }>("sync_app_icon", { foreground, force });
      if (request === sequence) publish({ busy: false, error: Boolean(result.warning), message: result.warning || `App icons synced. ${result.shortcutsUpdated} shortcut${result.shortcutsUpdated === 1 ? "" : "s"} refreshed.` });
    } catch (error) {
      if (request === sequence) publish({ busy: false, error: true, message: `Icon sync failed. Try Refresh desktop icon. ${String(error)}` });
    }
  });
  return pending;
}
export function AppIconSync() {
  const { settings } = useAppearance();
  const dark = useSystemDark();
  const palette = resolvedIconPalette(settings, dark);
  const foreground = settings.syncAppIcon ? palette.foreground : DEFAULT_DARK_COLORS.route;
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    const timer = setTimeout(() => void refreshAppIcon(foreground, false), 400);
    return () => clearTimeout(timer);
  }, [foreground]);
  return null;
}
