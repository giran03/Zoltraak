import { useEffect, useState } from "react";
import { extensionTheme, resolvedTheme, useAppearance } from "./useAppearance";

/** Publish the palette and geometry, through the desktop's authenticated API bridge. */
export function ExtensionThemeSync() {
  const { settings } = useAppearance();
  const [systemDark, setSystemDark] = useState(() => matchMedia("(prefers-color-scheme: dark)").matches);
  useEffect(() => {
    const media = matchMedia("(prefers-color-scheme: dark)");
    const change = () => setSystemDark(media.matches);
    media.addEventListener("change", change);
    return () => media.removeEventListener("change", change);
  }, []);
  // Corner radius and density travel with the palette, so the guide and the
  // extension pick them up from the same bridge and the same sync switch.
  const geometry = { radius: settings.radius, density: settings.density };
  const synced = extensionTheme(settings, systemDark);
  const payload = JSON.stringify({
    extension: synced ? { ...synced, geometry } : null,
    guide: { ...resolvedTheme(settings, systemDark), geometry },
  });
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let delivered = false;
    let disposed = false;
    async function publish() {
      if (delivered || disposed) return;
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        if (disposed) return;
        const themes = JSON.parse(payload);
        await invoke("set_extension_theme", { theme: themes.extension, guideTheme: themes.guide });
        delivered = true;
        clearInterval(retry);
      } catch { /* Retry when the native API finishes starting. */ }
    }
    const debounce = setTimeout(() => void publish(), 100);
    // Only until the first delivery; after that the timer has nothing to do.
    const retry = setInterval(() => void publish(), 3000);
    return () => { disposed = true; clearTimeout(debounce); clearInterval(retry); };
  }, [payload]);
  return null;
}
