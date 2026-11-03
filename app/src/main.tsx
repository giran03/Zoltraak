import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ExtensionThemeSync } from "./lib/ExtensionThemeSync";
import { AppIconSync } from "./lib/AppIconSync";
import { bootstrapAppearance } from "./lib/useAppearance";
import "./styles.css";

// Applied before the first paint so the panel never flashes the wrong
// enamel or customization settings on launch.
bootstrapAppearance();

// Suppress the webview menu without interrupting the app's context-menu handlers.
document.addEventListener("contextmenu", event => event.preventDefault());

const root = document.getElementById("root");
if (!root) throw new Error("#root missing from index.html");

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <ExtensionThemeSync />
    <AppIconSync />
    <App />
  </React.StrictMode>,
);
