/**
 * Placement Controller for Zoltraak Browser Extension
 * Manages overlay preset selection, manual X/Y offsets, real-time live preview,
 * interactive dragging, tactile D-pad nudging, and chrome.storage synchronization.
 */

export const OVERLAY_PRESETS = [
  // Inside Playback
  { id: "top-right", label: "Top Right", group: "inside", desc: "Top-right corner of player (Default)" },
  { id: "top-left", label: "Top Left", group: "inside", desc: "Top-left corner of player" },
  { id: "top-center", label: "Top Center", group: "inside", desc: "Top center along upper edge" },
  { id: "bottom-right", label: "Bottom Right", group: "inside", desc: "Bottom-right corner above scrubber controls" },
  { id: "bottom-left", label: "Bottom Left", group: "inside", desc: "Bottom-left corner above player controls" },
  { id: "bottom-center", label: "Bottom Center", group: "inside", desc: "Bottom center above player controls" },
  // Outside Playback (Zero Occlusion)
  { id: "outside-top-right", label: "Outside Top R", group: "outside", desc: "Above player on the right (Zero occlusion)" },
  { id: "outside-top-left", label: "Outside Top L", group: "outside", desc: "Above player on the left (Zero occlusion)" },
  { id: "outside-bottom-right", label: "Outside Btm R", group: "outside", desc: "Below player on the right (Zero occlusion)" },
  { id: "outside-bottom-left", label: "Outside Btm L", group: "outside", desc: "Below player on the left (Zero occlusion)" },
];

export const DEFAULT_POSITION = {
  preset: "top-right",
  offsetX: 0,
  offsetY: 0,
  scale: 100,
};

function sanitizeScale(value) {
  const scale = Number(value);
  return Number.isFinite(scale) ? Math.max(50, Math.min(200, scale)) : 100;
}

/**
 * Normalizes and clamps position values.
 */
export function sanitizePosition(raw) {
  const preset = OVERLAY_PRESETS.some((p) => p.id === raw?.preset)
    ? raw.preset
    : DEFAULT_POSITION.preset;
  const offsetX = Math.max(-150, Math.min(150, Number(raw?.offsetX) || 0));
  const offsetY = Math.max(-150, Math.min(150, Number(raw?.offsetY) || 0));
  const scale = sanitizeScale(raw?.scale);
  return { preset, offsetX, offsetY, scale };
}

export function previewPlacementStyles(raw) {
  const { preset, offsetX, offsetY, scale } = sanitizePosition(raw);
  const isOutside = preset.startsWith("outside-");
  const isBottom = preset.includes("bottom");
  const isLeft = preset.includes("left");
  const isCenter = preset.includes("center");
  const anchorBottom = preset.startsWith("outside-top") || (!isOutside && isBottom);
  const styles = {
    top: "",
    bottom: "",
    left: "",
    right: "",
    transform: "",
    transformOrigin: `${isLeft ? "left" : isCenter ? "center" : "right"} ${anchorBottom ? "bottom" : "top"}`,
    scale: String(scale / 100),
  };

  if (preset.startsWith("outside-top")) styles.bottom = `calc(100% + 8px - ${offsetY}px)`;
  else if (preset.startsWith("outside-bottom")) styles.top = `calc(100% + 8px + ${offsetY}px)`;
  else if (isBottom) styles.bottom = `calc(48px - ${offsetY}px)`;
  else styles.top = `calc(12px + ${offsetY}px)`;

  if (isLeft) styles.left = `calc(12px + ${offsetX}px)`;
  else if (isCenter) {
    styles.left = `calc(50% + ${offsetX}px)`;
    styles.transform = "translateX(-50%)";
  } else styles.right = `calc(12px - ${offsetX}px)`;

  return styles;
}

/**
 * Initializes the placement editor inside a parent DOM container.
 */
export function mountPlacementEditor(rootEl, options = {}) {
  let state = { ...DEFAULT_POSITION };
  let saveTimer = null;
  let isDragging = false;
  let dragStart = { x: 0, y: 0, startOffsetX: 0, startOffsetY: 0 };
  let nudgeStep = 1;

  // DOM element references
  const els = {
    previewStage: rootEl.querySelector(".preview-stage"),
    previewPlayer: rootEl.querySelector(".preview-player"),
    previewOverlay: rootEl.querySelector(".preview-overlay"),
    previewDropdown: rootEl.querySelector(".preview-dropdown"),
    toggleDropdownBtn: rootEl.querySelector(".preview-toggle-dropdown"),
    hudPreset: rootEl.querySelector(".hud-preset"),
    hudCoords: rootEl.querySelector(".hud-coords"),
    presetsInside: rootEl.querySelector(".presets-inside"),
    presetsOutside: rootEl.querySelector(".presets-outside"),
    sliderX: rootEl.querySelector(".slider-x"),
    inputX: rootEl.querySelector(".input-x"),
    sliderY: rootEl.querySelector(".slider-y"),
    inputY: rootEl.querySelector(".input-y"),
    sliderScale: rootEl.querySelector(".slider-scale"),
    inputScale: rootEl.querySelector(".input-scale"),
    resetBtn: rootEl.querySelector(".btn-reset-offsets"),
    stepToggleBtn: rootEl.querySelector(".btn-step-toggle"),
    dpadUp: rootEl.querySelector(".dpad-up"),
    dpadDown: rootEl.querySelector(".dpad-down"),
    dpadLeft: rootEl.querySelector(".dpad-left"),
    dpadRight: rootEl.querySelector(".dpad-right"),
    dpadCenter: rootEl.querySelector(".dpad-center"),
  };

  /**
   * Persists position to chrome.storage.local with debounce.
   */
  function persistState(instant = false) {
    if (saveTimer) clearTimeout(saveTimer);
    const doSave = async () => {
      try {
        if (chrome?.storage?.local) {
          await chrome.storage.local.set({ overlayPosition: state });
        }
      } catch (err) {
        console.warn("[OpenDM] Failed to save overlayPosition:", err);
      }
    };
    if (instant) {
      void doSave();
    } else {
      saveTimer = setTimeout(doSave, 80);
    }
  }

  /**
   * Updates all UI controls and the live preview stage.
   */
  function updateUI() {
    // 1. Update active preset button highlighting
    rootEl.querySelectorAll(".preset-chip").forEach((chip) => {
      const active = chip.dataset.preset === state.preset;
      chip.classList.toggle("active", active);
      chip.setAttribute("aria-pressed", active ? "true" : "false");
    });

    // 2. Update sliders and numeric inputs
    if (els.sliderX && els.sliderX.value !== String(state.offsetX)) {
      els.sliderX.value = state.offsetX;
    }
    if (els.inputX && els.inputX.value !== String(state.offsetX)) {
      els.inputX.value = state.offsetX;
    }
    if (els.sliderY && els.sliderY.value !== String(state.offsetY)) {
      els.sliderY.value = state.offsetY;
    }
    if (els.inputY && els.inputY.value !== String(state.offsetY)) {
      els.inputY.value = state.offsetY;
    }
    if (els.sliderScale && els.sliderScale.value !== String(state.scale)) {
      els.sliderScale.value = state.scale;
    }
    if (els.inputScale && els.inputScale.value !== String(state.scale)) {
      els.inputScale.value = state.scale;
    }

    // 3. Update HUD badge readout
    const activePresetObj = OVERLAY_PRESETS.find((p) => p.id === state.preset);
    if (els.hudPreset) {
      els.hudPreset.textContent = activePresetObj ? activePresetObj.label : state.preset;
    }
    if (els.hudCoords) {
      const signX = state.offsetX > 0 ? `+${state.offsetX}` : `${state.offsetX}`;
      const signY = state.offsetY > 0 ? `+${state.offsetY}` : `${state.offsetY}`;
      els.hudCoords.textContent = `X: ${signX}px · Y: ${signY}px · ${state.scale}%`;
    }

    // 4. Update the live preview positioning
    updatePreviewPlacement();
  }

  /** Positions the simulated overlay relative to the 16:9 preview player. */
  function updatePreviewPlacement() {
    if (!els.previewOverlay || !els.previewPlayer) return;

    const preset = state.preset;
    const isOutside = preset.startsWith("outside-");
    const isBottom = preset.includes("bottom");
    const isLeft = preset.includes("left");
    const isCenter = preset.includes("center");

    // Reset placement classes
    els.previewOverlay.className = "preview-overlay";
    els.previewOverlay.classList.toggle("is-outside", isOutside);
    els.previewOverlay.classList.toggle("is-bottom", isBottom);
    els.previewOverlay.classList.toggle("is-left", isLeft);
    els.previewOverlay.classList.toggle("is-center", isCenter);

    // Flow direction for format dropdown
    if (els.previewDropdown) {
      els.previewDropdown.classList.toggle("flow-up", isBottom);
    }

    Object.assign(els.previewOverlay.style, previewPlacementStyles(state));
  }

  /**
   * Renders the preset buttons into their respective containers.
   */
  function renderPresets() {
    if (!els.presetsInside || !els.presetsOutside) return;

    els.presetsInside.textContent = "";
    els.presetsOutside.textContent = "";

    for (const preset of OVERLAY_PRESETS) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "preset-chip";
      btn.dataset.preset = preset.id;
      btn.title = preset.desc;

      // Small tactical icon depicting position relative to rectangle
      const icon = createPresetMiniGlyph(preset.id);
      const label = document.createElement("span");
      label.className = "preset-label";
      label.textContent = preset.label;

      btn.append(icon, label);
      btn.addEventListener("click", () => {
        state.preset = preset.id;
        updateUI();
        persistState(true);
      });

      if (preset.group === "outside") {
        els.presetsOutside.appendChild(btn);
      } else {
        els.presetsInside.appendChild(btn);
      }
    }
  }

  /**
   * Helper to create a clean geometric SVG glyph representing the position.
   */
  function createPresetMiniGlyph(id) {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("width", "16");
    svg.setAttribute("height", "13");
    svg.setAttribute("viewBox", "0 0 20 16");
    svg.setAttribute("fill", "none");
    svg.setAttribute("stroke", "currentColor");
    svg.setAttribute("stroke-width", "1.2");
    svg.setAttribute("aria-hidden", "true");

    // Video frame box
    const rect = document.createElementNS("http://www.w3.org/2000/svg", "rect");
    rect.setAttribute("x", "3");
    rect.setAttribute("y", "3");
    rect.setAttribute("width", "14");
    rect.setAttribute("height", "10");
    rect.setAttribute("rx", "1");
    rect.setAttribute("stroke", "currentColor");
    rect.setAttribute("opacity", "0.6");
    svg.appendChild(rect);

    // Indicator dot/box
    const dot = document.createElementNS("http://www.w3.org/2000/svg", "rect");
    dot.setAttribute("width", "4");
    dot.setAttribute("height", "3");
    dot.setAttribute("rx", "0.5");
    dot.setAttribute("fill", "var(--clear, #35c46f)");
    dot.setAttribute("stroke", "none");

    if (id === "top-right") {
      dot.setAttribute("x", "12");
      dot.setAttribute("y", "4");
    } else if (id === "top-left") {
      dot.setAttribute("x", "4");
      dot.setAttribute("y", "4");
    } else if (id === "top-center") {
      dot.setAttribute("x", "8");
      dot.setAttribute("y", "4");
    } else if (id === "bottom-right") {
      dot.setAttribute("x", "12");
      dot.setAttribute("y", "9");
    } else if (id === "bottom-left") {
      dot.setAttribute("x", "4");
      dot.setAttribute("y", "9");
    } else if (id === "bottom-center") {
      dot.setAttribute("x", "8");
      dot.setAttribute("y", "9");
    } else if (id === "outside-top-right") {
      dot.setAttribute("x", "12");
      dot.setAttribute("y", "0.5");
    } else if (id === "outside-top-left") {
      dot.setAttribute("x", "4");
      dot.setAttribute("y", "0.5");
    } else if (id === "outside-bottom-right") {
      dot.setAttribute("x", "12");
      dot.setAttribute("y", "12.5");
    } else if (id === "outside-bottom-left") {
      dot.setAttribute("x", "4");
      dot.setAttribute("y", "12.5");
    }
    svg.appendChild(dot);
    return svg;
  }

  /**
   * Bind events to sliders, inputs, nudgers, and drag handles.
   */
  function bindEvents() {
    // X slider & input
    els.sliderX?.addEventListener("input", (e) => {
      state.offsetX = Math.max(-150, Math.min(150, Number(e.target.value) || 0));
      updateUI();
      persistState();
    });
    els.inputX?.addEventListener("change", (e) => {
      state.offsetX = Math.max(-150, Math.min(150, Number(e.target.value) || 0));
      updateUI();
      persistState(true);
    });

    // Y slider & input
    els.sliderY?.addEventListener("input", (e) => {
      state.offsetY = Math.max(-150, Math.min(150, Number(e.target.value) || 0));
      updateUI();
      persistState();
    });
    els.inputY?.addEventListener("change", (e) => {
      state.offsetY = Math.max(-150, Math.min(150, Number(e.target.value) || 0));
      updateUI();
      persistState(true);
    });

    // Overlay scale slider & input
    els.sliderScale?.addEventListener("input", (e) => {
      state.scale = sanitizeScale(e.target.value);
      updateUI();
      persistState();
    });
    els.inputScale?.addEventListener("change", (e) => {
      state.scale = sanitizeScale(e.target.value);
      updateUI();
      persistState(true);
    });

    // Reset button
    els.resetBtn?.addEventListener("click", () => {
      state = { ...DEFAULT_POSITION };
      updateUI();
      persistState(true);
    });

    // Step toggle button (1px vs 5px)
    els.stepToggleBtn?.addEventListener("click", () => {
      nudgeStep = nudgeStep === 1 ? 5 : 1;
      els.stepToggleBtn.textContent = `±${nudgeStep}px`;
      els.stepToggleBtn.setAttribute("aria-label", `Nudge step ${nudgeStep}px`);
    });

    // D-pad nudgers
    const applyNudge = (dx, dy) => {
      state.offsetX = Math.max(-150, Math.min(150, state.offsetX + dx));
      state.offsetY = Math.max(-150, Math.min(150, state.offsetY + dy));
      updateUI();
      persistState(true);
    };

    els.dpadUp?.addEventListener("click", () => applyNudge(0, -nudgeStep));
    els.dpadDown?.addEventListener("click", () => applyNudge(0, nudgeStep));
    els.dpadLeft?.addEventListener("click", () => applyNudge(-nudgeStep, 0));
    els.dpadRight?.addEventListener("click", () => applyNudge(nudgeStep, 0));
    els.dpadCenter?.addEventListener("click", () => {
      state.offsetX = 0;
      state.offsetY = 0;
      updateUI();
      persistState(true);
    });

    // Dropdown sample preview toggle
    els.toggleDropdownBtn?.addEventListener("click", () => {
      if (els.previewDropdown) {
        const isOpen = els.previewDropdown.classList.toggle("open");
        els.toggleDropdownBtn.classList.toggle("active", isOpen);
        els.toggleDropdownBtn.setAttribute("aria-expanded", isOpen ? "true" : "false");
      }
    });

    // Drag-to-position on preview overlay
    if (els.previewOverlay) {
      els.previewOverlay.addEventListener("pointerdown", (e) => {
        // Prevent drag when clicking preview buttons
        if (e.target.closest("button")) return;
        isDragging = true;
        els.previewOverlay.setPointerCapture(e.pointerId);
        dragStart = {
          x: e.clientX,
          y: e.clientY,
          startOffsetX: state.offsetX,
          startOffsetY: state.offsetY,
        };
        els.previewOverlay.classList.add("dragging");
      });

      els.previewOverlay.addEventListener("pointermove", (e) => {
        if (!isDragging) return;
        const deltaX = Math.round(e.clientX - dragStart.x);
        const deltaY = Math.round(e.clientY - dragStart.y);
        state.offsetX = Math.max(-150, Math.min(150, dragStart.startOffsetX + deltaX));
        state.offsetY = Math.max(-150, Math.min(150, dragStart.startOffsetY + deltaY));
        updateUI();
        persistState();
      });

      const endDrag = () => {
        if (isDragging) {
          isDragging = false;
          els.previewOverlay.classList.remove("dragging");
          persistState(true);
        }
      };

      els.previewOverlay.addEventListener("pointerup", endDrag);
      els.previewOverlay.addEventListener("pointercancel", endDrag);
    }
  }

  /**
   * Listen to external storage changes (from another tab or popup).
   */
  if (chrome?.storage?.onChanged) {
    chrome.storage.onChanged.addListener((changes, area) => {
      if (area === "local" && changes.overlayPosition) {
        state = sanitizePosition(changes.overlayPosition.newValue);
        updateUI();
      }
    });
  }

  /**
   * Public initialize function.
   */
  async function init() {
    renderPresets();
    bindEvents();

    // Load stored position
    if (chrome?.storage?.local) {
      try {
        const res = await chrome.storage.local.get("overlayPosition");
        if (res.overlayPosition) {
          state = sanitizePosition(res.overlayPosition);
        }
      } catch (err) {
        console.warn("[OpenDM] Failed to load overlayPosition:", err);
      }
    }

    updateUI();
  }

  void init();

  return {
    getState: () => ({ ...state }),
    setState: (next) => {
      state = sanitizePosition(next);
      updateUI();
      persistState(true);
    },
  };
}
