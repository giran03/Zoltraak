import React from "react";
import ReactDOM from "react-dom/client";
import { bootstrapAppearance } from "./lib/useAppearance";
import "./styles.css";

// Applied before the first paint so the panel never flashes the wrong
// enamel or customization settings on launch.
bootstrapAppearance();

// Suppress the webview menu without interrupting the app's context-menu handlers.
document.addEventListener("contextmenu", event => event.preventDefault());

const root = document.getElementById("root");
if (!root) throw new Error("#root missing from index.html");

// The tray icon's menu is a second, tiny window running this same page. It
// only needs the menu, so it never loads the app or its background syncs.
const label = (window as unknown as { __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } } })
  .__TAURI_INTERNALS__?.metadata?.currentWindow?.label;

async function mount(container: HTMLElement) {
  if (label === "tray-menu") {
    const { default: TrayMenu } = await import("./TrayMenu");
    ReactDOM.createRoot(container).render(<React.StrictMode><TrayMenu /></React.StrictMode>);
    return;
  }
  const [{ default: App }, { ExtensionThemeSync }, { AppIconSync }] = await Promise.all([
    import("./App"),
    import("./lib/ExtensionThemeSync"),
    import("./lib/AppIconSync"),
  ]);
  ReactDOM.createRoot(container).render(
    <React.StrictMode>
      <ExtensionThemeSync />
      <AppIconSync />
      <App />
    </React.StrictMode>,
  );
}

void mount(root);
