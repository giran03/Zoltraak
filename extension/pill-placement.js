// Placement and appearance settings for the Dispatch Board pill.
//
// Shared by the content script (which draws the pill) and the placement page
// (which edits it), so both read one definition of what a valid setting is.
// Pure functions only: nothing here touches the DOM or extension storage.
(() => {
  const ANCHORS = [
    "top-left", "top-center", "top-right",
    "middle-left", "middle-right",
    "bottom-left", "bottom-center", "bottom-right",
  ];
  const SIZES = ["compact", "standard", "detailed"];
  const CORNERS = ["top-left", "top-right", "bottom-left", "bottom-right"];

  const DEFAULTS = Object.freeze({
    anchor: "bottom-left",
    offsetX: 0,
    offsetY: 0,
    margin: 20,
    scale: 100,
    idleOpacity: 100,
    size: "standard",
    hideWhenIdle: false,
    frosted: false,
    autoDodge: true,
    perSite: false,
  });

  const LIMITS = Object.freeze({
    margin: [0, 80],
    scale: [75, 150],
    idleOpacity: [30, 100],
    offset: [-2000, 2000],
  });

  /** A dropped pill within this many pixels of an anchor's resting place snaps onto it. */
  const MAGNET = 24;
  /** Half the collapsed pill's height: a middle anchor centres the pill, not the whole panel. */
  const MIDDLE_HALF = 17;

  const clamp = (value, [min, max], fallback) => {
    const number = Number(value);
    return Number.isFinite(number) ? Math.max(min, Math.min(max, Math.round(number))) : fallback;
  };

  const flag = (value, fallback) => (typeof value === "boolean" ? value : fallback);

  /**
   * Normalise anything read from storage. `legacyCorner` is the old single
   * `pillCorner` value, honoured only when no placement has been saved yet.
   */
  function sanitize(raw, legacyCorner) {
    const source = raw && typeof raw === "object" ? raw : {};
    const legacy = !raw && CORNERS.includes(legacyCorner) ? legacyCorner : null;
    return {
      anchor: ANCHORS.includes(source.anchor) ? source.anchor : legacy || DEFAULTS.anchor,
      offsetX: clamp(source.offsetX, LIMITS.offset, DEFAULTS.offsetX),
      offsetY: clamp(source.offsetY, LIMITS.offset, DEFAULTS.offsetY),
      margin: clamp(source.margin, LIMITS.margin, DEFAULTS.margin),
      scale: clamp(source.scale, LIMITS.scale, DEFAULTS.scale),
      idleOpacity: clamp(source.idleOpacity, LIMITS.idleOpacity, DEFAULTS.idleOpacity),
      size: SIZES.includes(source.size) ? source.size : DEFAULTS.size,
      hideWhenIdle: flag(source.hideWhenIdle, DEFAULTS.hideWhenIdle),
      frosted: flag(source.frosted, DEFAULTS.frosted),
      autoDodge: flag(source.autoDodge, DEFAULTS.autoDodge),
      perSite: flag(source.perSite, DEFAULTS.perSite),
    };
  }

  /** Per-site spots only carry where the pill sits, never how it looks. */
  function sanitizeSite(raw) {
    const full = sanitize(raw);
    return { anchor: full.anchor, offsetX: full.offsetX, offsetY: full.offsetY };
  }

  function sanitizeSites(raw) {
    const sites = {};
    if (!raw || typeof raw !== "object") return sites;
    for (const [host, value] of Object.entries(raw)) {
      if (typeof host === "string" && host && host.length <= 253 && value && typeof value === "object") {
        sites[host] = sanitizeSite(value);
      }
    }
    return sites;
  }

  /** The placement in force on one site: its own spot when per-site is on and one is saved. */
  function resolveForSite(settings, sites, host) {
    const base = sanitize(settings);
    const own = base.perSite && host ? sites?.[host] : null;
    return own ? { ...base, ...sanitizeSite(own) } : base;
  }

  function layout(anchor) {
    const [row, column] = anchor.includes("middle")
      ? ["middle", anchor.endsWith("left") ? "left" : "right"]
      : anchor.split("-");
    return { v: row, h: column };
  }

  /** Where the fixed-position host sits for a placement. Offsets: right and down are positive. */
  function hostStyle(placement) {
    const p = sanitize(placement);
    const { v, h } = layout(p.anchor);
    const style = { top: "auto", right: "auto", bottom: "auto", left: "auto", transform: "none" };
    if (h === "left") style.left = `${p.margin + p.offsetX}px`;
    else if (h === "right") style.right = `${p.margin - p.offsetX}px`;
    else {
      style.left = `calc(50% + ${p.offsetX}px)`;
      style.transform = "translateX(-50%)";
    }
    if (v === "top") style.top = `${p.margin + p.offsetY}px`;
    else if (v === "bottom") style.bottom = `${p.margin - p.offsetY}px`;
    else style.top = `calc(50% + ${p.offsetY}px)`;
    return style;
  }

  /** Scale grows away from the anchored edge so the margin holds. */
  function transformOrigin(anchor) {
    const { v, h } = layout(anchor);
    return `${h} ${v === "bottom" ? "bottom" : "top"}`;
  }

  /** Which anchor a point on screen is closest to: thirds, with the middle cell going to the nearest edge. */
  function zoneAnchor(x, y, width, height) {
    const column = x < width / 3 ? 0 : x > (2 * width) / 3 ? 2 : 1;
    const row = y < height / 3 ? 0 : y > (2 * height) / 3 ? 2 : 1;
    if (row === 1 && column === 1) {
      // Fractions of the screen, so a wide window does not always favour top and bottom.
      const edges = [
        [x / width, "middle-left"],
        [(width - x) / width, "middle-right"],
        [y / height, "top-center"],
        [(height - y) / height, "bottom-center"],
      ];
      return edges.reduce((best, edge) => (edge[0] < best[0] ? edge : best))[1];
    }
    return [
      ["top-left", "top-center", "top-right"],
      ["middle-left", "", "middle-right"],
      ["bottom-left", "bottom-center", "bottom-right"],
    ][row][column];
  }

  const magnet = (offset) => (Math.abs(offset) <= MAGNET ? 0 : offset);

  /**
   * Turn a drop into a placement: the anchor nearest the pointer, plus how far
   * the pill landed from that anchor's resting place (zero when close enough).
   */
  function placementFromDrop({ rect, pointer, viewport, margin = DEFAULTS.margin }) {
    const anchor = zoneAnchor(pointer.x, pointer.y, viewport.width, viewport.height);
    const { v, h } = layout(anchor);
    const centreX = rect.left + (rect.right - rect.left) / 2;
    const rawX = h === "left" ? rect.left - margin
      : h === "right" ? rect.right - (viewport.width - margin)
      : centreX - viewport.width / 2;
    const rawY = v === "top" ? rect.top - margin
      : v === "bottom" ? rect.bottom - (viewport.height - margin)
      : rect.top - (viewport.height / 2 - MIDDLE_HALF);
    return {
      anchor,
      offsetX: clamp(magnet(rawX), LIMITS.offset, 0),
      offsetY: clamp(magnet(rawY), LIMITS.offset, 0),
    };
  }

  const BACKUP_KEYS = ["captureAll", "enableSfx", "showMediaButton", "showSessionPill", "overlayPosition", "pillPlacement", "pillSites"];
  const BACKUP_MARKER = "zoltraak-extension-settings";

  /** Settings worth carrying to another browser. The pairing token is never among them. */
  function buildBackup(stored) {
    const settings = {};
    for (const key of BACKUP_KEYS) if (stored && stored[key] !== undefined) settings[key] = stored[key];
    return { format: BACKUP_MARKER, version: 1, settings };
  }

  /**
   * Validate an exported file before anything is written back. Unknown keys
   * are dropped and every value is re-sanitised, so a hand-edited or hostile
   * file can only ever produce valid settings.
   */
  function parseBackup(text, sanitizeOverlayPosition) {
    let data;
    try { data = JSON.parse(text); } catch { return { ok: false, error: "That file is not valid JSON." }; }
    if (!data || data.format !== BACKUP_MARKER || typeof data.settings !== "object" || !data.settings) {
      return { ok: false, error: "That file is not a Zoltraak settings export." };
    }
    const input = data.settings;
    const values = {};
    for (const key of ["captureAll", "enableSfx", "showMediaButton", "showSessionPill"]) {
      if (typeof input[key] === "boolean") values[key] = input[key];
    }
    if (input.overlayPosition && typeof sanitizeOverlayPosition === "function") {
      values.overlayPosition = sanitizeOverlayPosition(input.overlayPosition);
    }
    if (input.pillPlacement) values.pillPlacement = sanitize(input.pillPlacement);
    if (input.pillSites) values.pillSites = sanitizeSites(input.pillSites);
    return Object.keys(values).length
      ? { ok: true, values }
      : { ok: false, error: "That file has no settings Zoltraak recognises." };
  }

  globalThis.ZoltraakPillPlacement = Object.freeze({
    ANCHORS, SIZES, DEFAULTS, LIMITS, MAGNET, MIDDLE_HALF, BACKUP_KEYS,
    sanitize, sanitizeSite, sanitizeSites, resolveForSite, layout, hostStyle,
    transformOrigin, zoneAnchor, placementFromDrop, buildBackup, parseBackup,
  });
})();
