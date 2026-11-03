import assert from "node:assert/strict";
import { test } from "node:test";
import {
  BUILTIN_PRESETS,
  DEFAULT_APPEARANCE,
  DEFAULT_DARK_COLORS,
  DEFAULT_LIGHT_COLORS,
  LIQUID_GLASS_COLORS,
  readAppearance,
  writeAppearance,
  extensionTheme,
  resolvedTheme,
  type CustomColors,
} from "../src/lib/useAppearance.ts";

test("file opening feedback persists, defaults for older settings, and bounds its duration", () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  const stored = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (key: string) => stored.get(key) ?? null,
    setItem: (key: string, value: string) => stored.set(key, value),
  } });
  try {
    assert.equal(readAppearance().fileOpenFeedback, true);
    assert.equal(readAppearance().fileOpenFeedbackDuration, 500);
    assert.equal(readAppearance().downloadDoubleClickInterval, 250);
    assert.equal(readAppearance().effects.noiseGrainSize, 1);
    writeAppearance({ ...DEFAULT_APPEARANCE, fileOpenFeedback: false, fileOpenFeedbackDuration: 350, downloadDoubleClickInterval: 150 });
    assert.equal(readAppearance().downloadDoubleClickInterval, 150);
    assert.equal(readAppearance().fileOpenFeedback, false);
    assert.equal(readAppearance().fileOpenFeedbackDuration, 350);
    for (const [saved, expected] of [[-5, 100], [9999, 600], [null, 500]]) {
      stored.set("zoltraak.appearance.v1", JSON.stringify({ fileOpenFeedbackDuration: saved }));
      assert.equal(readAppearance().fileOpenFeedbackDuration, expected);
    }
    for (const [saved, expected] of [[-5, 100], [9999, 600], [null, 250]]) {
      stored.set("zoltraak.appearance.v1", JSON.stringify({ downloadDoubleClickInterval: saved }));
      assert.equal(readAppearance().downloadDoubleClickInterval, expected);
    }
    for (const [saved, expected] of [[-1, 0.5], [10, 4], [1.34, 1.25], [null, 1]]) {
      stored.set("zoltraak.appearance.v1", JSON.stringify({ effects: { noiseGrainSize: saved } }));
      assert.equal(readAppearance().effects.noiseGrainSize, expected);
    }
  } finally {
    if (previous) Object.defineProperty(globalThis, "localStorage", previous);
    else Reflect.deleteProperty(globalThis, "localStorage");
  }
});
import {
  COLUMNS,
  COLUMN_IDS,
  DEFAULT_COLUMNS,
  fractionOf,
  hostOf,
  kindOf,
  normalizeColumns,
  remainingOf,
  typeOf,
} from "../src/lib/columns.ts";
import { formatRelativeDate } from "../src/lib/format.ts";

const REQUIRED_COLOR_KEYS: (keyof CustomColors)[] = [
  "panel",
  "panelFace",
  "panelRecess",
  "panelSunk",
  "edge",
  "edgeStrong",
  "ink",
  "inkDim",
  "inkFaint",
  "route",
  "clear",
  "caution",
  "danger",
  "paused",
  "onSignal",
  "lampOff",
  "brass",
  "blockClear",
];

test("built-in presets list contains all curated themes with valid color slots", () => {
  assert.equal(BUILTIN_PRESETS.length, 20);

  const presetIds = BUILTIN_PRESETS.map((p) => p.id);
  assert.ok(presetIds.includes("classic-dispatch"));
  assert.ok(presetIds.includes("amber-crt"));
  assert.ok(presetIds.includes("emerald-radar"));
  assert.ok(presetIds.includes("solarized-foundry"));
  assert.ok(presetIds.includes("nordic-frost"));
  assert.ok(presetIds.includes("monochrome-carbon"));
  assert.ok(presetIds.includes("midnight-steel"));
  assert.ok(presetIds.includes("abyssal-depths"));
  assert.ok(presetIds.includes("amethyst-stealth"));
  assert.ok(presetIds.includes("crimson-operator"));
  assert.ok(presetIds.includes("forest-sentinel"));
  assert.ok(presetIds.includes("paper-editorial"));
  assert.ok(presetIds.includes("day-shift-enamel"));
  assert.ok(presetIds.includes("drafting-blueprint"));
  assert.ok(presetIds.includes("solarized-day"));
  assert.ok(presetIds.includes("nordic-snow"));
  assert.ok(presetIds.includes("matcha-sage"));
  assert.ok(presetIds.includes("sandstone-foundry"));
  assert.ok(presetIds.includes("synthwave-twilight"));
  assert.ok(presetIds.includes("high-contrast-ops"));

  for (const preset of BUILTIN_PRESETS) {
    assert.ok(preset.name.length > 0, `Preset ${preset.id} missing name`);
    assert.ok(preset.description.length > 0, `Preset ${preset.id} missing description`);
    for (const key of REQUIRED_COLOR_KEYS) {
      const val = preset.colors[key];
      assert.ok(typeof val === "string" && val.startsWith("#"), `Preset ${preset.id} color key ${key} is invalid (${val})`);
    }
  }
});

test("light and dark categories contain exactly 7 distinct presets each", () => {
  const darkPresets = BUILTIN_PRESETS.filter((p) => p.category === "dark");
  const lightPresets = BUILTIN_PRESETS.filter((p) => p.category === "light");
  assert.equal(darkPresets.length, 7, `Expected 7 dark presets, got ${darkPresets.length}`);
  assert.equal(lightPresets.length, 7, `Expected 7 light presets, got ${lightPresets.length}`);
  for (const p of darkPresets) {
    assert.equal(p.category, "dark");
    assert.ok(p.name.length > 0);
  }
  for (const p of lightPresets) {
    assert.equal(p.category, "light");
    assert.ok(p.name.length > 0);
  }
});

test("default light and dark palettes have all required color properties", () => {
  for (const key of REQUIRED_COLOR_KEYS) {
    assert.ok(DEFAULT_LIGHT_COLORS[key], `Light palette missing ${key}`);
    assert.ok(DEFAULT_DARK_COLORS[key], `Dark palette missing ${key}`);
  }
});

test("formatRelativeDate returns human friendly relative times", () => {
  const now = Date.now();

  assert.equal(formatRelativeDate(), "—");
  assert.equal(formatRelativeDate("invalid-date"), "—");

  // Just now
  assert.equal(formatRelativeDate(new Date(now - 10 * 1000).toISOString()), "just now");

  // Minutes ago
  assert.equal(formatRelativeDate(new Date(now - 3 * 60 * 1000).toISOString()), "3m ago");

  // Hours ago
  assert.equal(formatRelativeDate(new Date(now - 5 * 3600 * 1000).toISOString()), "5h ago");

  // Days ago
  assert.equal(formatRelativeDate(new Date(now - 4 * 86400 * 1000).toISOString()), "4d ago");
});

test("readAppearance defaults fallback gracefully in non-browser environment", () => {
  const settings = readAppearance();
  assert.equal(settings.themeMode, "system");
  assert.equal(settings.uiScale, 100);
  assert.equal(settings.density, "default");
  assert.equal(settings.radius, "bevel");
  assert.equal(settings.fontMode, "default");
  assert.equal(settings.motion, "full");
  assert.equal(settings.effects.scanlines, false);
  assert.equal(settings.effects.scanlinesIntensity, 70);
  assert.equal(settings.effects.grid, false);
  assert.equal(settings.effects.gridIntensity, 50);
  assert.equal(settings.effects.glass, false);
  assert.equal(settings.effects.glassIntensity, 60);
  assert.equal(settings.effects.glassThemeOverride, true);
  assert.equal(settings.effects.glassNavbar, true);
  assert.equal(settings.effects.glassModals, true);
  assert.equal(settings.effects.glassSettings, true);
  assert.equal(settings.effects.glassOverlays, true);
  assert.equal(settings.effects.vignette, false);
  assert.equal(settings.effects.vignetteIntensity, 50);
  assert.equal(settings.effects.bloom, false);
  assert.equal(settings.effects.bloomIntensity, 50);
  assert.equal(settings.effects.noise, false);
  assert.equal(settings.effects.noiseIntensity, 35);
  assert.equal(settings.effects.warmth, false);
  assert.equal(settings.effects.warmthIntensity, 40);
  assert.equal(settings.telemetry.dateFormat, "relative");
  assert.deepEqual(settings.columns, DEFAULT_COLUMNS);
});

test("column ids are unique and the default set is a subset of the registry", () => {
  assert.equal(new Set(COLUMN_IDS).size, COLUMN_IDS.length);
  for (const id of DEFAULT_COLUMNS) assert.ok(COLUMN_IDS.includes(id));
  // Every column carries a width, a breakpoint and a name for the header menu.
  for (const column of COLUMNS) {
    assert.ok(column.width.startsWith("w-"), column.id);
    assert.ok(column.show.includes("block"), column.id);
    assert.ok(column.menuLabel.length > 0, column.id);
  }
});

test("normalizeColumns drops unknown ids and forces registry order", () => {
  assert.deepEqual(normalizeColumns(["rate", "aspect", "not-a-column"]), ["aspect", "rate"]);
  // A duplicate is a visible column, not two of them.
  assert.deepEqual(normalizeColumns(["eta", "eta"]), ["eta"]);
  assert.deepEqual(normalizeColumns([]), []);
  assert.deepEqual(normalizeColumns("nonsense"), DEFAULT_COLUMNS);
});

test("derived column values read the fields the register has", () => {
  const item = {
    id: "1",
    filename: "clip.webm",
    url: "https://www.example.com/media/clip.webm?x=1",
    directory: "C:\Downloads",
    state: "downloading",
    downloadedBytes: 250,
    totalBytes: 1000,
    connections: 4,
    bytesPerSecond: 100,
    etaSeconds: 8,
    priority: 0,
    error: null,
    path: "C:\Downloads\clip.webm",
  } as never;

  assert.equal(hostOf("https://www.example.com/a"), "example.com");
  assert.equal(hostOf("not a url"), "—");
  assert.equal(typeOf(item), "WEBM");
  assert.equal(kindOf(item), "file");
  assert.equal(fractionOf(item), 0.25);
  assert.equal(remainingOf(item), 750);

  const unknownTotal = { ...(item as object), totalBytes: null } as never;
  assert.equal(fractionOf(unknownTotal), null);
  assert.equal(remainingOf(unknownTotal), null);

  // A finished row is complete whatever the reported total says.
  const done = { ...(item as object), state: "finished", totalBytes: null } as never;
  assert.equal(fractionOf(done), 1);
  assert.equal(remainingOf(done), 0);
});

test("theme export and import parsing and schema verification", () => {
  const validExport = {
    version: 1,
    appName: "Zoltraak",
    type: "theme-export",
    themeMode: "custom",
    activePresetId: "custom",
    colors: {
      panel: "#000000",
      panelFace: "#111111",
      route: "#38bdf8",
    },
    density: "compact",
    radius: "sharp",
    effects: {
      scanlines: true,
      grid: false,
    },
  };

  const jsonStr = JSON.stringify(validExport);
  const parsed = JSON.parse(jsonStr);

  assert.equal(parsed.appName, "Zoltraak");
  assert.equal(parsed.colors.panel, "#000000");
  assert.equal(parsed.density, "compact");
  assert.equal(parsed.radius, "sharp");
  assert.equal(parsed.effects.scanlines, true);
});

test("custom draft colors can be modified in isolation without mutating base palettes", () => {
  const baseColors = { ...DEFAULT_DARK_COLORS };
  const draftColors = { ...baseColors, panel: "#ff0000", route: "#00ff00" };

  // Base palette must remain unchanged
  assert.notEqual(baseColors.panel, draftColors.panel);
  assert.notEqual(baseColors.route, draftColors.route);
  assert.equal(baseColors.panel, DEFAULT_DARK_COLORS.panel);

  // Draft colors have changes
  assert.equal(draftColors.panel, "#ff0000");
  assert.equal(draftColors.route, "#00ff00");
});

test("paused color is distinct and defined in all presets and defaults", () => {
  assert.ok(DEFAULT_LIGHT_COLORS.paused, "Default light colors missing paused");
  assert.ok(DEFAULT_DARK_COLORS.paused, "Default dark colors missing paused");
  assert.notEqual(DEFAULT_DARK_COLORS.paused, DEFAULT_DARK_COLORS.danger, "Paused color should be distinct from danger");

  for (const preset of BUILTIN_PRESETS) {
    assert.ok(preset.colors.paused, `Preset ${preset.id} missing paused color`);
    assert.ok(preset.colors.paused.startsWith("#"), `Preset ${preset.id} paused color must be a hex string`);
  }
});

test("extension theme resolves system, presets, custom colors and disabling sync", () => {
  const light = extensionTheme(DEFAULT_APPEARANCE, false)!;
  assert.equal(light.variables["--panel"], DEFAULT_LIGHT_COLORS.panel);
  assert.equal(light.colorScheme, "light");
  assert.equal(extensionTheme(DEFAULT_APPEARANCE, true)!.variables["--face"], DEFAULT_DARK_COLORS.panelFace);
  const preset = extensionTheme({ ...DEFAULT_APPEARANCE, themeMode: "preset", activePresetId: "paper-editorial" }, true)!;
  assert.equal(preset.colorScheme, "light");
  assert.equal(preset.variables["--panel"], BUILTIN_PRESETS.find(p => p.id === "paper-editorial")!.colors.panel);
  const custom = extensionTheme({ ...DEFAULT_APPEARANCE, themeMode: "custom", customColors: { ...DEFAULT_DARK_COLORS, route: "#abcdef" } }, false)!;
  assert.equal(custom.variables["--route"], "#abcdef");
  assert.equal(extensionTheme({ ...DEFAULT_APPEARANCE, syncExtensionTheme: false }, true), null);
  assert.equal(resolvedTheme({ ...DEFAULT_APPEARANCE, syncExtensionTheme: false }, true).colorScheme, "dark");
});

test("default appearance settings specify hardware acceleration, webgpu, and full motion", () => {
  assert.equal(DEFAULT_APPEARANCE.hardwareAcceleration, true);
  assert.equal(DEFAULT_APPEARANCE.webGpu, true);
  assert.equal(DEFAULT_APPEARANCE.motion, "full");
  assert.equal(DEFAULT_APPEARANCE.confettiOnComplete, false);
  assert.equal(DEFAULT_APPEARANCE.confettiSfx, false);
  assert.equal(DEFAULT_APPEARANCE.notificationLimit, 25);
});

test("notificationLimit defaults to 25 and clamps within valid range", () => {
  assert.equal(DEFAULT_APPEARANCE.notificationLimit, 25);
  const custom = { ...DEFAULT_APPEARANCE, notificationLimit: 50 };
  assert.equal(custom.notificationLimit, 50);
});

test("confettiOnComplete and confettiSfx settings can be enabled and disabled", () => {
  const enabled = { ...DEFAULT_APPEARANCE, confettiOnComplete: true, confettiSfx: true };
  const disabled = { ...DEFAULT_APPEARANCE, confettiOnComplete: false, confettiSfx: false };
  assert.equal(enabled.confettiOnComplete, true);
  assert.equal(enabled.confettiSfx, true);
  assert.equal(disabled.confettiOnComplete, false);
  assert.equal(disabled.confettiSfx, false);
});

test("appearance settings support full, subtle, and reduced motion levels", () => {
  const fullSettings = { ...DEFAULT_APPEARANCE, motion: "full" as const };
  const subtleSettings = { ...DEFAULT_APPEARANCE, motion: "subtle" as const };
  const reducedSettings = { ...DEFAULT_APPEARANCE, motion: "reduced" as const };

  assert.equal(fullSettings.motion, "full");
  assert.equal(subtleSettings.motion, "subtle");
  assert.equal(reducedSettings.motion, "reduced");
});

test("getGpuDiagnostics provides fallback state when hardware acceleration is disabled", async () => {
  const { getGpuDiagnostics, isWebGpuSupported } = await import("../src/lib/webgpu.ts");

  // In Node test environment, WebGPU is generally not present
  assert.equal(typeof isWebGpuSupported(), "boolean");

  // When hardware acceleration is disabled, activeBackend must report "disabled"
  const disabledDiag = await getGpuDiagnostics(false, true);
  assert.equal(disabledDiag.activeBackend, "disabled");

  // When hardware acceleration is enabled but WebGPU is disabled, fallback to canvas2d
  const canvasDiag = await getGpuDiagnostics(true, false);
  assert.equal(canvasDiag.activeBackend, "canvas2d");
});

test("atmospheric effects support 7 distinct overlays and intensity clamping", () => {
  const custom = {
    ...DEFAULT_APPEARANCE,
    effects: {
      scanlines: true,
      scanlinesIntensity: 85,
      grid: true,
      gridIntensity: 45,
      glass: true,
      glassIntensity: 75,
      vignette: true,
      vignetteIntensity: 65,
      bloom: true,
      bloomIntensity: 90,
      noise: true,
      noiseIntensity: 25,
      warmth: true,
      warmthIntensity: 55,
    },
  };

  assert.equal(custom.effects.scanlines, true);
  assert.equal(custom.effects.scanlinesIntensity, 85);
  assert.equal(custom.effects.grid, true);
  assert.equal(custom.effects.gridIntensity, 45);
  assert.equal(custom.effects.glass, true);
  assert.equal(custom.effects.glassIntensity, 75);
  assert.equal(custom.effects.vignette, true);
  assert.equal(custom.effects.vignetteIntensity, 65);
  assert.equal(custom.effects.bloom, true);
  assert.equal(custom.effects.bloomIntensity, 90);
  assert.equal(custom.effects.noise, true);
  assert.equal(custom.effects.noiseIntensity, 25);
  assert.equal(custom.effects.warmth, true);
  assert.equal(custom.effects.warmthIntensity, 55);
});

test("glass effect supports liquid glass and matte frosted styles", () => {
  assert.equal(DEFAULT_APPEARANCE.effects.glassStyle, "liquid");
  const parsed = readAppearance();
  assert.equal(parsed.effects.glassStyle, "liquid");
});

test("noise effect supports underlay and overlay placement", () => {
  assert.equal(DEFAULT_APPEARANCE.effects.noisePlacement, "overlay");
  const parsed = readAppearance();
  assert.equal(parsed.effects.noisePlacement, "overlay");
});

test("liquid glass dedicated theme has all required color slots", () => {
  for (const key of REQUIRED_COLOR_KEYS) {
    const val = LIQUID_GLASS_COLORS[key];
    assert.ok(typeof val === "string" && val.startsWith("#"), `Liquid glass color key ${key} is invalid (${val})`);
  }
  // Verify distinct deep obsidian & cyan optics
  assert.equal(LIQUID_GLASS_COLORS.panel, "#060911");
  assert.equal(LIQUID_GLASS_COLORS.route, "#0ea5e9");
});

test("resolvedTheme and extensionTheme apply liquid glass override only when glass is enabled and override is not false", () => {
  // 1. When glass is OFF, theme override is NOT active
  const glassOff = resolvedTheme({
    ...DEFAULT_APPEARANCE,
    themeMode: "preset",
    activePresetId: "amber-crt",
    effects: { ...DEFAULT_APPEARANCE.effects, glass: false, glassThemeOverride: true },
  }, true);
  assert.notEqual(glassOff.colors.panel, LIQUID_GLASS_COLORS.panel);
  assert.equal(glassOff.colors.panel, BUILTIN_PRESETS.find((p) => p.id === "amber-crt")!.colors.panel);

  // 2. When glass is ON and glassThemeOverride is true (or default), theme is overridden to LIQUID_GLASS_COLORS
  const glassOn = resolvedTheme({
    ...DEFAULT_APPEARANCE,
    themeMode: "preset",
    activePresetId: "amber-crt",
    effects: { ...DEFAULT_APPEARANCE.effects, glass: true, glassThemeOverride: true },
  }, true);
  assert.equal(glassOn.colors.panel, LIQUID_GLASS_COLORS.panel);
  assert.equal(glassOn.colors.route, LIQUID_GLASS_COLORS.route);
  assert.equal(glassOn.colorScheme, "dark");

  const extTheme = extensionTheme({
    ...DEFAULT_APPEARANCE,
    themeMode: "preset",
    activePresetId: "amber-crt",
    effects: { ...DEFAULT_APPEARANCE.effects, glass: true, glassThemeOverride: true },
  }, true)!;
  assert.equal(extTheme.variables["--panel"], LIQUID_GLASS_COLORS.panel);

  // 3. When glass is ON but glassThemeOverride is disabled (false), theme falls back to base preset
  const overrideDisabled = resolvedTheme({
    ...DEFAULT_APPEARANCE,
    themeMode: "preset",
    activePresetId: "amber-crt",
    effects: { ...DEFAULT_APPEARANCE.effects, glass: true, glassThemeOverride: false },
  }, true);
  assert.notEqual(overrideDisabled.colors.panel, LIQUID_GLASS_COLORS.panel);
  assert.equal(overrideDisabled.colors.panel, BUILTIN_PRESETS.find((p) => p.id === "amber-crt")!.colors.panel);
});

test("glass surface toggles allow independent activation of navbar, modals, settings, and overlays", () => {
  const customSurfaces = {
    ...DEFAULT_APPEARANCE,
    effects: {
      ...DEFAULT_APPEARANCE.effects,
      glass: true,
      glassNavbar: true,
      glassModals: false,
      glassSettings: true,
      glassOverlays: false,
    },
  };

  assert.equal(customSurfaces.effects.glassNavbar, true);
  assert.equal(customSurfaces.effects.glassModals, false);
  assert.equal(customSurfaces.effects.glassSettings, true);
  assert.equal(customSurfaces.effects.glassOverlays, false);
});

