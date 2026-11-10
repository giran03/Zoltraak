import { useEffect, useRef } from "react";
import { applyAppearance, readAppearance } from "./lib/useAppearance";

type TrayAction = "show" | "folder" | "quit" | "close";

const ITEMS: { action: TrayAction; label: string; danger?: boolean; icon: string }[] = [
  { action: "show", label: "Show Zoltraak", icon: "M4 5h16v14H4zM4 9h16" },
  { action: "folder", label: "Open Downloads Folder", icon: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" },
  { action: "quit", label: "Quit Zoltraak", danger: true, icon: "M12 3v9M6.3 7.3a8 8 0 1 0 11.4 0" },
];

async function invokeNative(command: string, args?: Record<string, unknown>): Promise<void> {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke(command, args);
}

/** Re-read the saved appearance so the menu always matches the main window. */
function syncTheme() {
  applyAppearance(readAppearance());
  // The menu's native window is a fixed size, so UI scale must not resize it.
  (document.documentElement.style as CSSStyleDeclaration & { zoom?: string }).zoom = "";
}

/** The tray icon's right-click menu: the app's own page, hosted in a small frameless window. */
export default function TrayMenu() {
  const first = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    syncTheme();
    void invokeNative("tray_menu_ready").catch(() => { /* Opened outside the desktop shell. */ });

    // The window is reused between openings and its webview may have been
    // suspended, so refresh the theme every time it is shown.
    const refresh = () => { syncTheme(); first.current?.focus(); };
    const onStorage = (event: StorageEvent) => { if (event.key?.startsWith("zoltraak.")) syncTheme(); };
    window.addEventListener("storage", onStorage);
    window.addEventListener("focus", refresh);
    const media = matchMedia("(prefers-color-scheme: dark)");
    media.addEventListener("change", syncTheme);

    let unlisten: (() => void) | undefined;
    let disposed = false;
    if ("__TAURI_INTERNALS__" in window) {
      void import("@tauri-apps/api/event")
        .then(({ listen }) => listen("tray-menu-shown", refresh))
        .then((stop) => { if (disposed) stop(); else unlisten = stop; })
        .catch(() => { /* The focus handler covers this. */ });
    }
    return () => {
      disposed = true;
      window.removeEventListener("storage", onStorage);
      window.removeEventListener("focus", refresh);
      media.removeEventListener("change", syncTheme);
      unlisten?.();
    };
  }, []);

  const choose = (action: TrayAction) => { void invokeNative("tray_menu_action", { action }).catch(() => undefined); };

  const onKeyDown = (event: React.KeyboardEvent) => {
    const buttons = Array.from(document.querySelectorAll<HTMLButtonElement>("[data-tray-item]"));
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") choose("close");
    else if (event.key === "ArrowDown") buttons[(index + 1) % buttons.length]?.focus();
    else if (event.key === "ArrowUp") buttons[(index - 1 + buttons.length) % buttons.length]?.focus();
    else return;
    event.preventDefault();
  };

  return (
    <div
      role="menu"
      aria-label="Zoltraak"
      onKeyDown={onKeyDown}
      className="fixed inset-0 flex flex-col gap-0.5 overflow-hidden border border-edge-strong bg-panel p-1.5 text-ink select-none"
    >
      <div className="flex items-center gap-2 px-2.5 pb-1.5 pt-1">
        <span className="size-1.5 rounded-full bg-clear" aria-hidden />
        <span className="font-mono text-[10px] font-semibold uppercase tracking-[0.18em] text-ink-faint">Zoltraak</span>
      </div>
      {ITEMS.map((item, index) => (
        <div key={item.action} className="contents">
          {item.danger && <div className="mx-1 my-1 h-px bg-edge" role="separator" />}
          <button
            ref={index === 0 ? first : undefined}
            data-tray-item
            role="menuitem"
            type="button"
            onClick={() => choose(item.action)}
            className={`flex h-9 w-full items-center gap-2.5 px-2.5 text-left text-[13px] font-medium outline-none transition-colors duration-100 hover:bg-face focus-visible:bg-face ${
              item.danger ? "text-danger" : "text-ink"
            }`}
            style={{ borderRadius: "var(--hw-r, 4px)" }}
          >
            <svg viewBox="0 0 24 24" className="size-4 shrink-0 opacity-80" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
              <path d={item.icon} />
            </svg>
            {item.label}
          </button>
        </div>
      ))}
    </div>
  );
}
