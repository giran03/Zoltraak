import "./mock";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "../src/App";
import { ExtensionThemeSync } from "../src/lib/ExtensionThemeSync";
import { AppIconSync } from "../src/lib/AppIconSync";
import { bootstrapAppearance } from "../src/lib/useAppearance";
import "../src/styles.css";

bootstrapAppearance();
document.addEventListener("contextmenu", event => event.preventDefault());
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode><ExtensionThemeSync /><AppIconSync /><App /></React.StrictMode>,
);
