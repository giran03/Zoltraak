import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { hasEngine } from "../lib/native";

/** Routes close/minimize through Tauri so the existing tray preferences apply. */
export function WindowControls() {
  const desktop = hasEngine();
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!desktop) return;
    const appWindow = getCurrentWindow();
    let disposed = false;
    const refresh = () => {
      void appWindow.isMaximized().then(value => {
        if (!disposed) setMaximized(value);
      }).catch(console.error);
    };
    refresh();
    const subscription = appWindow.onResized(refresh).catch(error => { console.error(error); });
    return () => {
      disposed = true;
      void subscription.then(unlisten => unlisten?.()).catch(console.error);
    };
  }, [desktop]);

  if (!desktop) return null;
  const buttonClass = "focus-ring flex w-10 shrink-0 items-center justify-center text-ink-dim hover:bg-recess hover:text-ink";
  return (
    <div className="window-controls relative z-10 flex shrink-0 self-stretch border-l border-edge" role="group" aria-label="Window controls">
      <button type="button" aria-label="Minimize window" title="Minimize" className={buttonClass}
        onClick={() => void getCurrentWindow().minimize().catch(console.error)}>
        <svg aria-hidden="true" width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor"><path d="M1 6.5h10" /></svg>
      </button>
      <button type="button" aria-label={maximized ? "Restore window" : "Maximize window"} title={maximized ? "Restore" : "Maximize"} className={buttonClass}
        onClick={() => void getCurrentWindow().toggleMaximize().catch(console.error)}>
        <svg aria-hidden="true" width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor">
          {maximized ? <><path d="M3.5 3.5v-2h7v7h-2" /><rect x="1.5" y="3.5" width="7" height="7" /></> : <rect x="1.5" y="1.5" width="9" height="9" />}
        </svg>
      </button>
      <button type="button" aria-label="Close window" title="Close" className="focus-ring flex w-10 shrink-0 items-center justify-center text-ink-dim hover:bg-danger hover:text-white"
        onClick={() => void getCurrentWindow().close().catch(console.error)}>
        <svg aria-hidden="true" width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor"><path d="m1.5 1.5 9 9m0-9-9 9" /></svg>
      </button>
    </div>
  );
}
