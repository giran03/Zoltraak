import { mountPlacementEditor, sanitizePosition } from "./placement-controller.js";
import { mountPillEditor } from "./pill-controller.js";

const TABS = [
  { id: "tabVideo", panel: "placementRoot", title: "OVERLAY PLACEMENT & SCALE", hash: "" },
  { id: "tabPill", panel: "pillRoot", title: "DISPATCH PILL", hash: "#pill" },
];

function selectTab(hash) {
  const active = TABS.find((tab) => tab.hash === hash) || TABS[0];
  for (const tab of TABS) {
    const on = tab === active;
    document.getElementById(tab.id)?.setAttribute("aria-selected", String(on));
    const panel = document.getElementById(tab.panel);
    if (panel) panel.hidden = !on;
  }
  const title = document.querySelector(".page-title");
  if (title) title.textContent = active.title;
}

document.addEventListener("DOMContentLoaded", () => {
  const root = document.getElementById("placementRoot");
  if (root) mountPlacementEditor(root);
  const pillRoot = document.getElementById("pillRoot");
  if (pillRoot) mountPillEditor(pillRoot, { sanitizeOverlayPosition: sanitizePosition });

  for (const tab of TABS) {
    document.getElementById(tab.id)?.addEventListener("click", () => {
      history.replaceState(null, "", tab.hash || location.pathname);
      selectTab(tab.hash);
    });
  }
  selectTab(location.hash);
});
