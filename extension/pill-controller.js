/**
 * Dispatch Board pill editor for the placement page.
 *
 * Edits the `pillPlacement` and `pillSites` storage keys that content.js reads,
 * with a live preview drawn from the same placement maths the page uses, and
 * the settings backup (export, import, reset) for the extension as a whole.
 */

const SIM = { width: 960, height: 600 };
const LABELS = {
  "top-left": "Top left", "top-center": "Top centre", "top-right": "Top right",
  "middle-left": "Middle left", "middle-right": "Middle right",
  "bottom-left": "Bottom left", "bottom-center": "Bottom centre", "bottom-right": "Bottom right",
};
const SIZE_LABELS = { compact: "Compact", standard: "Standard", detailed: "Detailed" };
const SIZE_HINTS = {
  compact: "Icon and count only",
  standard: "Icon, state and count",
  detailed: "Adds overall progress",
};
const CLEARABLE_KEYS = ["pillCorner"];

function h(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value == null || value === false) continue;
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : String(value));
  }
  for (const child of children.flat()) {
    if (child != null) node.append(child);
  }
  return node;
}

function region(title, ...extra) {
  return h("div", { class: "region" }, h("span", { class: "legend", text: title }), h("span", { class: "rule" }), ...extra);
}

/** A tiny diagram of a window with the pill at an anchor. */
function anchorGlyph(anchor) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("width", "28");
  svg.setAttribute("height", "18");
  svg.setAttribute("viewBox", "0 0 28 18");
  svg.setAttribute("aria-hidden", "true");
  const frame = document.createElementNS(ns, "rect");
  for (const [k, v] of Object.entries({ x: 0.5, y: 0.5, width: 27, height: 17, rx: 1.5, fill: "none", stroke: "currentColor", "stroke-width": 1 })) frame.setAttribute(k, v);
  const mark = document.createElementNS(ns, "rect");
  const { v, h: column } = globalThis.ZoltraakPillPlacement.layout(anchor);
  const x = column === "left" ? 3 : column === "right" ? 28 - 3 - 9 : 9.5;
  const y = v === "top" ? 3 : v === "bottom" ? 18 - 3 - 4 : 7;
  for (const [k, val] of Object.entries({ x, y, width: 9, height: 4, rx: 1, fill: "currentColor" })) mark.setAttribute(k, val);
  svg.append(frame, mark);
  return svg;
}

export function mountPillEditor(root, { sanitizeOverlayPosition } = {}) {
  const PP = globalThis.ZoltraakPillPlacement;
  if (!PP || !root) return;

  let placement = PP.sanitize(undefined);
  let sites = {};
  let saveTimer = null;
  let simScale = 1;

  const status = h("p", { class: "describer pill-status", role: "status", "aria-live": "polite" });
  const say = (message) => { status.textContent = message; };

  // ------------------------------------------------------------- persistence
  function persist(instant = false) {
    if (saveTimer) clearTimeout(saveTimer);
    const save = () => chrome.storage.local.set({ pillPlacement: placement, pillSites: sites })
      .catch((error) => console.warn("[Zoltraak] Could not save pill placement:", error));
    if (instant) void save();
    else saveTimer = setTimeout(save, 80);
  }

  function update(patch, instant = false) {
    placement = PP.sanitize({ ...placement, ...patch });
    render();
    persist(instant);
  }

  // ----------------------------------------------------------------- preview
  const simPill = h("div", { class: "pill-sim-pill", tabindex: "0", role: "img", "aria-label": "Dispatch Board pill preview" });
  const simHost = h("div", { class: "pill-sim-host" }, simPill);
  const sim = h("div", { class: "pill-sim", "aria-hidden": "true" },
    h("div", { class: "pill-sim-bar" }),
    h("div", { class: "pill-sim-line wide" }), h("div", { class: "pill-sim-line" }), h("div", { class: "pill-sim-line short" }),
    h("div", { class: "pill-sim-video" }),
    simHost);
  const stage = h("div", { class: "pill-stage" }, sim);

  function renderPreview() {
    const style = PP.hostStyle(placement);
    Object.assign(simHost.style, style);
    const { v } = PP.layout(placement.anchor);
    simPill.style.transform = `scale(${placement.scale / 100})`;
    simPill.style.transformOrigin = PP.transformOrigin(placement.anchor);
    simPill.style.marginTop = v === "middle" ? `-${PP.MIDDLE_HALF}px` : "0";
    simPill.style.opacity = String(placement.idleOpacity / 100);
    simPill.classList.toggle("frosted", placement.frosted);
    simPill.classList.toggle("compact", placement.size === "compact");
    simPill.textContent = "";
    simPill.append(
      h("span", { class: "lamp clear" }),
      placement.size === "compact" ? null : h("span", { class: "pill-sim-count", text: placement.size === "detailed" ? "2 running · 42%" : "2 running" }),
      h("span", { class: "pill-sim-badge", text: "5" }),
    );
  }

  function fitStage() {
    simScale = stage.clientWidth / SIM.width || 1;
    sim.style.transform = `scale(${simScale})`;
    stage.style.height = `${Math.round(SIM.height * simScale)}px`;
  }
  new ResizeObserver(fitStage).observe(stage);

  // Drag the preview pill; it snaps the way the real pill does.
  simPill.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    simPill.setPointerCapture(event.pointerId);
    const stageRect = stage.getBoundingClientRect();
    const hostRect = simHost.getBoundingClientRect();
    const startX = event.clientX;
    const startY = event.clientY;
    const originLeft = (hostRect.left - stageRect.left) / simScale;
    const originTop = (hostRect.top - stageRect.top) / simScale;
    const pillRect = simPill.getBoundingClientRect();
    const size = { width: pillRect.width / simScale, height: pillRect.height / simScale };
    Object.assign(simHost.style, { left: `${originLeft}px`, top: `${originTop}px`, right: "auto", bottom: "auto", transform: "none" });

    const move = (e) => {
      const left = Math.max(0, Math.min(SIM.width - size.width, originLeft + (e.clientX - startX) / simScale));
      const top = Math.max(0, Math.min(SIM.height - size.height, originTop + (e.clientY - startY) / simScale));
      simHost.style.left = `${left}px`;
      simHost.style.top = `${top}px`;
    };
    const finish = (e) => {
      simPill.removeEventListener("pointermove", move);
      simPill.removeEventListener("pointerup", finish);
      simPill.removeEventListener("pointercancel", finish);
      const rect = simPill.getBoundingClientRect();
      const local = {
        left: (rect.left - stageRect.left) / simScale, top: (rect.top - stageRect.top) / simScale,
        right: (rect.right - stageRect.left) / simScale, bottom: (rect.bottom - stageRect.top) / simScale,
      };
      const drop = PP.placementFromDrop({
        rect: local,
        pointer: { x: (e.clientX - stageRect.left) / simScale, y: (e.clientY - stageRect.top) / simScale },
        viewport: SIM,
        margin: placement.margin,
      });
      update(drop, true);
    };
    simPill.addEventListener("pointermove", move);
    simPill.addEventListener("pointerup", finish);
    simPill.addEventListener("pointercancel", finish);
  });

  // ---------------------------------------------------------------- controls
  const anchorButtons = new Map();
  const anchorGrid = h("div", { class: "preset-grid pill-anchor-grid", role: "radiogroup", "aria-label": "Pill anchor" });
  const gridOrder = ["top-left", "top-center", "top-right", "middle-left", null, "middle-right", "bottom-left", "bottom-center", "bottom-right"];
  for (const anchor of gridOrder) {
    if (!anchor) {
      anchorGrid.append(h("div", { class: "pill-anchor-page describer", text: "Page" }));
      continue;
    }
    const button = h("button", {
      type: "button", class: "preset-chip", role: "radio", title: LABELS[anchor],
      onclick: () => update({ anchor, offsetX: 0, offsetY: 0 }, true),
    }, anchorGlyph(anchor), h("span", { class: "preset-label", text: LABELS[anchor] }));
    anchorButtons.set(anchor, button);
    anchorGrid.append(button);
  }

  const sliders = new Map();
  function sliderRow(key, label, sub, [min, max], step, unit) {
    const range = h("input", { type: "range", min, max, step, "aria-label": label });
    const number = h("input", { type: "number", class: "describer", min, max, step, "aria-label": `${label} in ${unit}` });
    const commit = (event) => update({ [key]: event.target.value });
    range.addEventListener("input", commit);
    number.addEventListener("change", commit);
    sliders.set(key, [range, number]);
    return h("div", { class: "offset-row" },
      h("div", { class: "offset-label-col" }, h("span", { class: "legend", text: label }), h("span", { class: "offset-sub describer", text: sub })),
      h("div", { class: "offset-control-col" }, range, h("div", { class: "number-box" }, number, h("span", { class: "unit describer", text: unit }))));
  }

  const sizeButtons = new Map();
  const sizeGrid = h("div", { class: "preset-grid", role: "radiogroup", "aria-label": "Pill size" });
  for (const size of PP.SIZES) {
    const button = h("button", { type: "button", class: "preset-chip", role: "radio", title: SIZE_HINTS[size], onclick: () => update({ size }, true) },
      h("span", { class: "preset-label", text: SIZE_LABELS[size] }), h("span", { class: "describer", text: SIZE_HINTS[size] }));
    sizeButtons.set(size, button);
    sizeGrid.append(button);
  }

  const toggles = new Map();
  function toggleRow(key, label, hint) {
    const input = h("input", { type: "checkbox", id: `pill-${key}` });
    input.addEventListener("change", () => update({ [key]: input.checked }, true));
    toggles.set(key, input);
    return h("label", { class: "pill-toggle", for: `pill-${key}` }, input,
      h("span", {}, h("span", { class: "legend", text: label }), h("span", { class: "offset-sub describer", text: hint })));
  }

  const siteList = h("div", { class: "pill-sites" });
  function renderSites() {
    siteList.textContent = "";
    const hosts = Object.keys(sites).sort();
    siteList.hidden = !placement.perSite && hosts.length === 0;
    if (hosts.length === 0) {
      siteList.append(h("p", { class: "describer pill-empty", text: "No site-specific spots saved yet. Drag the pill on a page to save one." }));
      return;
    }
    for (const host of hosts) {
      siteList.append(h("div", { class: "pill-site" },
        h("span", { class: "describer", text: `${host} · ${LABELS[sites[host].anchor]}` }),
        h("button", {
          type: "button", class: "btn-reset-offsets describer", "aria-label": `Forget the spot for ${host}`, text: "Forget",
          onclick: () => { const { [host]: _gone, ...rest } = sites; sites = rest; render(); persist(true); say(`Forgot ${host}.`); },
        })));
    }
  }

  function render() {
    renderPreview();
    for (const [anchor, button] of anchorButtons) {
      const on = anchor === placement.anchor;
      button.classList.toggle("active", on);
      button.setAttribute("aria-checked", String(on));
    }
    for (const [size, button] of sizeButtons) {
      const on = size === placement.size;
      button.classList.toggle("active", on);
      button.setAttribute("aria-checked", String(on));
    }
    for (const [key, [range, number]] of sliders) {
      if (document.activeElement !== number) number.value = placement[key];
      range.value = placement[key];
    }
    for (const [key, input] of toggles) input.checked = placement[key];
    renderSites();
  }

  // ------------------------------------------------------------------ backup
  const fileInput = h("input", { type: "file", accept: "application/json,.json", hidden: true });
  fileInput.addEventListener("change", async () => {
    const file = fileInput.files?.[0];
    fileInput.value = "";
    if (!file) return;
    const result = PP.parseBackup(await file.text(), sanitizeOverlayPosition);
    if (!result.ok) { say(result.error); return; }
    await chrome.storage.local.set(result.values);
    say(`Imported ${Object.keys(result.values).length} setting${Object.keys(result.values).length === 1 ? "" : "s"}.`);
  });

  async function exportSettings() {
    const stored = await chrome.storage.local.get(PP.BACKUP_KEYS);
    const blob = new Blob([JSON.stringify(PP.buildBackup(stored), null, 2)], { type: "application/json" });
    const link = h("a", { href: URL.createObjectURL(blob), download: "zoltraak-extension-settings.json" });
    document.body.append(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(link.href), 1000);
    say("Settings exported. The pairing token is not included.");
  }

  async function resetAll() {
    if (!window.confirm("Reset every extension setting to its default? Your pairing with the app is kept.")) return;
    await chrome.storage.local.remove([...PP.BACKUP_KEYS, ...CLEARABLE_KEYS]);
    say("All extension settings are back to their defaults.");
  }

  // ------------------------------------------------------------------ layout
  const stageColumn = h("section", { class: "stage-column" },
    h("div", { class: "region", style: "padding-top: 0;" }, h("span", { class: "legend", text: "Live Preview" }), h("span", { class: "rule" })),
    h("div", { class: "preview-stage-wrap" }, stage),
    h("p", { class: "describer pill-hint", text: "Drag the pill anywhere. It snaps to the nearest anchor, or keeps its distance along the edge. Alt+Shift+P opens it on any page." }),
  );

  const controls = h("section", { class: "controls-column" },
    h("div", { class: "region", style: "padding-top: 0;" }, h("span", { class: "legend", text: "Anchor" }), h("span", { class: "rule" }),
      h("button", {
        type: "button", class: "btn-reset-offsets describer", text: "Reset placement",
        title: "Back to bottom left, default size and look; forgets per-site spots",
        onclick: () => { sites = {}; placement = PP.sanitize(undefined); render(); persist(true); say("Placement reset."); },
      })),
    anchorGrid,
    region("Size"), sizeGrid,
    region("Position & Look"),
    sliderRow("margin", "Edge Margin", "Gap from the window edge", PP.LIMITS.margin, 1, "px"),
    sliderRow("scale", "Pill Scale", "75% / 150%", PP.LIMITS.scale, 5, "%"),
    sliderRow("idleOpacity", "Idle Opacity", "Fade while nothing is downloading", PP.LIMITS.idleOpacity, 5, "%"),
    region("Behaviour"),
    toggleRow("hideWhenIdle", "Hide when idle", "Folds into a small edge tab a few seconds after the last download"),
    toggleRow("autoDodge", "Stay off player controls", "Lifts the pill clear of a playing video's control bar"),
    toggleRow("frosted", "Frosted background", "Blurs the page behind the pill"),
    toggleRow("perSite", "Remember a spot per site", "Dragging saves the position for this site only"),
    siteList,
    region("Settings Backup"),
    h("div", { class: "pill-backup" },
      h("button", { type: "button", class: "btn-reset-offsets describer", text: "Export settings", onclick: () => void exportSettings() }),
      h("button", { type: "button", class: "btn-reset-offsets describer", text: "Import settings", onclick: () => fileInput.click() }),
      h("button", { type: "button", class: "btn-reset-offsets describer pill-danger", text: "Reset all settings", onclick: () => void resetAll() }),
      fileInput),
    status,
  );

  root.append(stageColumn, controls);
  fitStage();

  // ---------------------------------------------------------------- lifecycle
  function load() {
    chrome.storage.local.get({ pillPlacement: null, pillSites: {}, pillCorner: null }).then((stored) => {
      placement = PP.sanitize(stored.pillPlacement, stored.pillCorner);
      sites = PP.sanitizeSites(stored.pillSites);
      render();
    });
  }
  load();

  // A drag on a real page, or an import, lands here without a reload.
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area === "local" && (changes.pillPlacement || changes.pillSites || changes.pillCorner)) load();
  });
}
