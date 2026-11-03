import { useCallback, useEffect, useSyncExternalStore } from "react";
// Extension-qualified: `node --test` runs these modules unbundled, and its
// resolver will not guess one.
import { DEFAULT_COLUMNS, normalizeColumns, type ColumnId } from "./columns.ts";
import { applyCustomFont, deleteCustomFontData, normalizeCustomFont, type CustomFontMeta } from "./customFont.ts";

export type ThemeMode = "system" | "light" | "dark" | "preset" | "custom";
export type Density = "compact" | "default" | "comfortable";
export type CornerRadius = "sharp" | "bevel" | "modern" | "rounded";
export type FontMode = "default" | "mono" | "system" | "sans" | "custom";

/** The presets that carry no uploaded file, and so can travel in a theme export. */
export const PRESET_FONT_MODES: readonly FontMode[] = ["default", "mono", "system", "sans"];
export type MotionMode = "full" | "subtle" | "reduced";

export interface IconPalette {
  useAppColors: boolean;
  foreground: string;
}
export function normalizeIconPalette(value: unknown): IconPalette | undefined {
  if (!value || typeof value !== "object") return undefined;
  const p = value as IconPalette;
  if (typeof p.useAppColors !== "boolean" || !/^#[a-f\d]{6}$/i.test(p.foreground)) return undefined;
  // Older palettes may contain a background; discard it during migration.
  return { useAppColors: p.useAppColors, foreground: p.foreground.toLowerCase() };
}

export interface CustomColors {
  panel: string;
  panelFace: string;
  panelRecess: string;
  panelSunk: string;
  edge: string;
  edgeStrong: string;
  ink: string;
  inkDim: string;
  inkFaint: string;
  route: string;
  clear: string;
  caution: string;
  danger: string;
  paused: string;
  onSignal: string;
  lampOff: string;
  brass: string;
  blockClear: string;
}

export interface ThemePreset {
  id: string;
  name: string;
  description: string;
  category: "classic" | "dark" | "light" | "retro" | "contrast";
  colors: CustomColors;
}

export interface UserPreset {
  iconPalette?: IconPalette;
  id: string;
  name: string;
  description?: string;
  colors: CustomColors;
  createdAt: number;
}

export interface AtmosphericEffects {
  scanlines: boolean;
  scanlinesIntensity: number;
  grid: boolean;
  gridIntensity: number;
  glass: boolean;
  glassIntensity: number;
  glassStyle?: "liquid" | "frosted";
  glassThemeOverride?: boolean;
  glassNavbar?: boolean;
  glassModals?: boolean;
  glassSettings?: boolean;
  glassOverlays?: boolean;
  vignette: boolean;
  vignetteIntensity: number;
  bloom: boolean;
  bloomIntensity: number;
  noise: boolean;
  noiseIntensity: number;
  noiseGrainSize: number;
  noisePlacement?: "underlay" | "overlay";
  warmth: boolean;
  warmthIntensity: number;
}

export interface TelemetryOptions {
  dateFormat: "relative" | "absolute";
  speedUnit: "MBps" | "Mbps";
}

/**
 * What the register showed before columns were configurable.
 *
 * Four booleans, each hard-wired to one cell. They survive only long enough to
 * translate a stored settings blob into a column set on first read.
 */
interface LegacyTelemetryOptions {
  showOccupancy?: boolean;
  showSpeedBadge?: boolean;
  showConnections?: boolean;
  showEta?: boolean;
}

export interface AppearanceSettings {
  syncAppIcon: boolean;
  iconPalettes: Record<string, IconPalette>;
  syncExtensionTheme: boolean;
  themeMode: ThemeMode;
  activePresetId: string;
  customColors: CustomColors;
  userPresets: UserPreset[];
  uiScale: number; // 80 to 125
  density: Density;
  radius: CornerRadius;
  fontMode: FontMode;
  /** The uploaded typeface, when one is stored; its file lives in IndexedDB. */
  customFont: CustomFontMeta | null;
  effects: AtmosphericEffects;
  motion: MotionMode;
  fileOpenFeedback: boolean;
  fileOpenFeedbackDuration: number;
  downloadDoubleClickInterval: number;
  hardwareAcceleration: boolean;
  webGpu: boolean;
  telemetry: TelemetryOptions;
  /** Visible register columns, by id. See `lib/columns`. */
  columns: ColumnId[];
  /** Trigger celebration confetti burst whenever a download completes. */
  confettiOnComplete: boolean;
  /** Play sound effect when confetti burst is triggered. */
  confettiSfx: boolean;
  /** Maximum number of notifications to keep in history (default: 25). */
  notificationLimit: number;
}

export const DEFAULT_LIGHT_COLORS: CustomColors = {
  panel: "#d7dcd2",
  panelFace: "#e6eae0",
  panelRecess: "#cdd3c7",
  panelSunk: "#bcc3b6",
  edge: "#a7afa0",
  edgeStrong: "#7b8476",
  ink: "#161a14",
  inkDim: "#3c4239",
  inkFaint: "#4f5749",
  route: "#1560c4",
  clear: "#086633",
  caution: "#7a4c00",
  danger: "#b01b18",
  paused: "#b84e06",
  onSignal: "#f4f6f2",
  lampOff: "#9aa294",
  brass: "#7d6d3f",
  blockClear: "#b3bbac",
};

export const DEFAULT_DARK_COLORS: CustomColors = {
  panel: "#161a17",
  panelFace: "#1e2320",
  panelRecess: "#101312",
  panelSunk: "#0b0e0d",
  edge: "#333b35",
  edgeStrong: "#4c564e",
  ink: "#e9ede7",
  inkDim: "#a6aea4",
  inkFaint: "#868e84",
  route: "#56a2f0",
  clear: "#35c46f",
  caution: "#e8a317",
  danger: "#f1595e",
  paused: "#ea6c24",
  onSignal: "#0b0e0d",
  lampOff: "#2b332d",
  brass: "#b09a63",
  blockClear: "#2b332d",
};

export const LIQUID_GLASS_COLORS: CustomColors = {
  panel: "#060911",
  panelFace: "#0c1322",
  panelRecess: "#04060c",
  panelSunk: "#020306",
  edge: "#1e2f4d",
  edgeStrong: "#325080",
  ink: "#f0f6fc",
  inkDim: "#9bb2d6",
  inkFaint: "#4e678f",
  route: "#0ea5e9",
  clear: "#10b981",
  caution: "#f59e0b",
  danger: "#f43f5e",
  paused: "#fb923c",
  onSignal: "#020408",
  lampOff: "#121b2b",
  brass: "#38bdf8",
  blockClear: "#121b2b",
};

export const BUILTIN_PRESETS: ThemePreset[] = [
  {
    id: "classic-dispatch",
    name: "Classic Dispatch",
    description: "Authentic industrial dispatch enamel with day and night shift illumination.",
    category: "classic",
    colors: DEFAULT_DARK_COLORS,
  },
  {
    id: "amber-crt",
    name: "Amber Phosphor",
    description: "Warm monochrome phosphor terminal on dark obsidian basalt.",
    category: "retro",
    colors: {
      panel: "#0c0b08",
      panelFace: "#171510",
      panelRecess: "#080705",
      panelSunk: "#050403",
      edge: "#382e18",
      edgeStrong: "#614e26",
      ink: "#fef3c7",
      inkDim: "#d4a373",
      inkFaint: "#8c6b45",
      route: "#f59e0b",
      clear: "#4ade80",
      caution: "#fbbf24",
      danger: "#f87171",
      paused: "#fb923c",
      onSignal: "#0c0b08",
      lampOff: "#241e12",
      brass: "#d4af37",
      blockClear: "#241e12",
    },
  },
  {
    id: "emerald-radar",
    name: "Emerald Radar",
    description: "Submarine navigation and military sonar console with glowing phosphor green telemetry.",
    category: "retro",
    colors: {
      panel: "#07120a",
      panelFace: "#0d1e12",
      panelRecess: "#040a06",
      panelSunk: "#020603",
      edge: "#183c24",
      edgeStrong: "#285e39",
      ink: "#d1fae5",
      inkDim: "#6ee7b7",
      inkFaint: "#3b7a57",
      route: "#10b981",
      clear: "#34d399",
      caution: "#fbbf24",
      danger: "#f43f5e",
      paused: "#fb923c",
      onSignal: "#040a06",
      lampOff: "#122619",
      brass: "#86efac",
      blockClear: "#122619",
    },
  },
  {
    id: "solarized-foundry",
    name: "Solarized Foundry",
    description: "Cast-iron locomotive console, polished brass gauges, and warm mechanical steam tone.",
    category: "retro",
    colors: {
      panel: "#14110f",
      panelFace: "#1c1815",
      panelRecess: "#0d0b09",
      panelSunk: "#080706",
      edge: "#3a2f26",
      edgeStrong: "#5c4a3b",
      ink: "#f5eee6",
      inkDim: "#c2b19f",
      inkFaint: "#7e6e5f",
      route: "#d97706",
      clear: "#22c55e",
      caution: "#eab308",
      danger: "#ef4444",
      paused: "#f97316",
      onSignal: "#0d0b09",
      lampOff: "#28211b",
      brass: "#e5b85a",
      blockClear: "#28211b",
    },
  },
  {
    id: "nordic-frost",
    name: "Nordic Frost",
    description: "Sub-zero glacial slate with crisp ice-cyan telemetry and frost-silver hairlines.",
    category: "dark",
    colors: {
      panel: "#0a0e17",
      panelFace: "#121929",
      panelRecess: "#060910",
      panelSunk: "#030509",
      edge: "#1f2d47",
      edgeStrong: "#324870",
      ink: "#f1f5f9",
      inkDim: "#94a3b8",
      inkFaint: "#64748b",
      route: "#38bdf8",
      clear: "#34d399",
      caution: "#fbbf24",
      danger: "#fb7185",
      paused: "#f97316",
      onSignal: "#060910",
      lampOff: "#1a263c",
      brass: "#7dd3fc",
      blockClear: "#1a263c",
    },
  },
  {
    id: "monochrome-carbon",
    name: "Monochrome Carbon",
    description: "Stealth matte carbon fiber with neutral gray hairlines and pure titanium white contrast.",
    category: "dark",
    colors: {
      panel: "#080808",
      panelFace: "#131313",
      panelRecess: "#040404",
      panelSunk: "#000000",
      edge: "#262626",
      edgeStrong: "#404040",
      ink: "#ffffff",
      inkDim: "#a3a3a3",
      inkFaint: "#737373",
      route: "#e5e5e5",
      clear: "#4ade80",
      caution: "#facc15",
      danger: "#f87171",
      paused: "#fb923c",
      onSignal: "#000000",
      lampOff: "#212121",
      brass: "#d4d4d4",
      blockClear: "#212121",
    },
  },
  {
    id: "midnight-steel",
    name: "Midnight Steel",
    description: "Deep gunmetal slate with cold steel hairlines and high-visibility electric cobalt routing.",
    category: "dark",
    colors: {
      panel: "#0f1318",
      panelFace: "#161c24",
      panelRecess: "#0a0d11",
      panelSunk: "#06080b",
      edge: "#242f3d",
      edgeStrong: "#38485c",
      ink: "#e2e8f0",
      inkDim: "#94a3b8",
      inkFaint: "#64748b",
      route: "#3b82f6",
      clear: "#22c55e",
      caution: "#eab308",
      danger: "#ef4444",
      paused: "#f97316",
      onSignal: "#06080b",
      lampOff: "#1e293b",
      brass: "#60a5fa",
      blockClear: "#1e293b",
    },
  },
  {
    id: "abyssal-depths",
    name: "Abyssal Depths",
    description: "Mariana oceanic abyssal navy with bioluminescent cyan telemetry and deep sea pressure plating.",
    category: "dark",
    colors: {
      panel: "#080d1a",
      panelFace: "#0f172a",
      panelRecess: "#050811",
      panelSunk: "#020408",
      edge: "#1e2d4a",
      edgeStrong: "#2e446e",
      ink: "#e0f2fe",
      inkDim: "#7dd3fc",
      inkFaint: "#38bdf8",
      route: "#0ea5e9",
      clear: "#10b981",
      caution: "#f59e0b",
      danger: "#f43f5e",
      paused: "#fb923c",
      onSignal: "#020408",
      lampOff: "#13213b",
      brass: "#0284c7",
      blockClear: "#13213b",
    },
  },
  {
    id: "amethyst-stealth",
    name: "Amethyst Stealth",
    description: "Dark obsidian violet with radiant lilac indicators and precision ruthenium boundaries.",
    category: "dark",
    colors: {
      panel: "#110d19",
      panelFace: "#191424",
      panelRecess: "#0c0912",
      panelSunk: "#07050a",
      edge: "#2d243e",
      edgeStrong: "#473961",
      ink: "#f3e8ff",
      inkDim: "#c084fc",
      inkFaint: "#7e699b",
      route: "#a855f7",
      clear: "#34d399",
      caution: "#fbbf24",
      danger: "#f43f5e",
      paused: "#fb923c",
      onSignal: "#07050a",
      lampOff: "#231b32",
      brass: "#d8b4fe",
      blockClear: "#231b32",
    },
  },
  {
    id: "crimson-operator",
    name: "Crimson Operator",
    description: "Tactical night-recon black with burnished blood-orange dials and incandescent ruby alerts.",
    category: "dark",
    colors: {
      panel: "#140d0e",
      panelFace: "#1c1214",
      panelRecess: "#0d0809",
      panelSunk: "#080405",
      edge: "#382024",
      edgeStrong: "#5c3037",
      ink: "#ffe4e6",
      inkDim: "#fda4af",
      inkFaint: "#9f656d",
      route: "#f43f5e",
      clear: "#22c55e",
      caution: "#eab308",
      danger: "#e11d48",
      paused: "#f97316",
      onSignal: "#080405",
      lampOff: "#29161a",
      brass: "#fb7185",
      blockClear: "#29161a",
    },
  },
  {
    id: "forest-sentinel",
    name: "Forest Sentinel",
    description: "Pine spruce tactical dark with moss green telemetry, olive hairlines, and jade beacon clear signals.",
    category: "dark",
    colors: {
      panel: "#0b120f",
      panelFace: "#121e19",
      panelRecess: "#070c0a",
      panelSunk: "#040706",
      edge: "#1f332a",
      edgeStrong: "#305243",
      ink: "#ecfdf5",
      inkDim: "#a7f3d0",
      inkFaint: "#527d6a",
      route: "#10b981",
      clear: "#34d399",
      caution: "#fbbf24",
      danger: "#f43f5e",
      paused: "#fb923c",
      onSignal: "#040706",
      lampOff: "#172720",
      brass: "#6ee7b7",
      blockClear: "#172720",
    },
  },
  {
    id: "paper-editorial",
    name: "Paper Editorial",
    description: "Calm archival paper, sepia etched boundaries, and deep royal blue fountain ink.",
    category: "light",
    colors: {
      panel: "#f5f3eb",
      panelFace: "#fdfcf7",
      panelRecess: "#eae6da",
      panelSunk: "#dcd7c8",
      edge: "#cbc4b2",
      edgeStrong: "#9c9480",
      ink: "#1c1917",
      inkDim: "#44403c",
      inkFaint: "#78716c",
      route: "#1d4ed8",
      clear: "#15803d",
      caution: "#b45309",
      danger: "#b91c1c",
      paused: "#c2410c",
      onSignal: "#ffffff",
      lampOff: "#b8b09d",
      brass: "#854d0e",
      blockClear: "#d3ccbb",
    },
  },
  {
    id: "day-shift-enamel",
    name: "Day Shift Enamel",
    description: "Crisp white industrial porcelain with gunmetal hairlines and locomotive route blue.",
    category: "light",
    colors: {
      panel: "#f1f5f9",
      panelFace: "#ffffff",
      panelRecess: "#e2e8f0",
      panelSunk: "#cbd5e1",
      edge: "#cbd5e1",
      edgeStrong: "#64748b",
      ink: "#0f172a",
      inkDim: "#334155",
      inkFaint: "#64748b",
      route: "#2563eb",
      clear: "#16a34a",
      caution: "#d97706",
      danger: "#dc2626",
      paused: "#b84e06",
      onSignal: "#ffffff",
      lampOff: "#94a3b8",
      brass: "#b45309",
      blockClear: "#cbd5e1",
    },
  },
  {
    id: "drafting-blueprint",
    name: "Drafting Blueprint",
    description: "Architectural drafting paper with technical cobalt rules and precision cyan accents.",
    category: "light",
    colors: {
      panel: "#edf4fa",
      panelFace: "#f8fbfe",
      panelRecess: "#dbe8f5",
      panelSunk: "#c8dcf0",
      edge: "#b6d1ed",
      edgeStrong: "#4a83be",
      ink: "#0e2a47",
      inkDim: "#26517a",
      inkFaint: "#5b84ad",
      route: "#0284c7",
      clear: "#059669",
      caution: "#d97706",
      danger: "#e11d48",
      paused: "#ea580c",
      onSignal: "#ffffff",
      lampOff: "#99bbde",
      brass: "#0284c7",
      blockClear: "#c5daf0",
    },
  },
  {
    id: "solarized-day",
    name: "Solarized Day",
    description: "Warm ivory parchment inspired by Solarized light with terracotta, olive, and teal signals.",
    category: "light",
    colors: {
      panel: "#fdf6e3",
      panelFace: "#eee8d5",
      panelRecess: "#e4dcbe",
      panelSunk: "#d8ceaf",
      edge: "#d2c8a7",
      edgeStrong: "#93a1a1",
      ink: "#073642",
      inkDim: "#586e75",
      inkFaint: "#839496",
      route: "#268bd2",
      clear: "#859900",
      caution: "#b58900",
      danger: "#dc322f",
      paused: "#cb4b16",
      onSignal: "#ffffff",
      lampOff: "#b4a889",
      brass: "#b58900",
      blockClear: "#d8ceaf",
    },
  },
  {
    id: "nordic-snow",
    name: "Nordic Snow",
    description: "Bright sub-zero glacial snowdrift with crisp ice-cyan telemetry and frost-silver hairlines.",
    category: "light",
    colors: {
      panel: "#f3f6f9",
      panelFace: "#ffffff",
      panelRecess: "#e5ebf2",
      panelSunk: "#d7e0eb",
      edge: "#c5d2e0",
      edgeStrong: "#708ba8",
      ink: "#1a2536",
      inkDim: "#41556e",
      inkFaint: "#738aab",
      route: "#0ea5e9",
      clear: "#10b981",
      caution: "#f59e0b",
      danger: "#f43f5e",
      paused: "#ea580c",
      onSignal: "#ffffff",
      lampOff: "#a4b7cc",
      brass: "#38bdf8",
      blockClear: "#d0dce8",
    },
  },
  {
    id: "matcha-sage",
    name: "Matcha Sage",
    description: "Calming botanical matcha porcelain with deep evergreen lettering and mint signals.",
    category: "light",
    colors: {
      panel: "#f2f6f3",
      panelFace: "#fafdfa",
      panelRecess: "#e3ede5",
      panelSunk: "#d5e3d8",
      edge: "#bdd2c2",
      edgeStrong: "#628a6c",
      ink: "#14291a",
      inkDim: "#2d4f37",
      inkFaint: "#5c7d65",
      route: "#2e7d32",
      clear: "#15803d",
      caution: "#b45309",
      danger: "#b91c1c",
      paused: "#c2410c",
      onSignal: "#ffffff",
      lampOff: "#9cb5a2",
      brass: "#857038",
      blockClear: "#cce0d1",
    },
  },
  {
    id: "sandstone-foundry",
    name: "Sandstone Foundry",
    description: "Warm desert sandstone with terracotta signal lights and milled copper trim.",
    category: "light",
    colors: {
      panel: "#f8f4ed",
      panelFace: "#fffdf9",
      panelRecess: "#ede3d4",
      panelSunk: "#dfd3c0",
      edge: "#d4c4af",
      edgeStrong: "#8c755d",
      ink: "#261f18",
      inkDim: "#4f4033",
      inkFaint: "#7d6957",
      route: "#c25e2e",
      clear: "#16a34a",
      caution: "#d97706",
      danger: "#dc2626",
      paused: "#c2410c",
      onSignal: "#ffffff",
      lampOff: "#bfae99",
      brass: "#b87333",
      blockClear: "#ded0bd",
    },
  },
  {
    id: "synthwave-twilight",
    name: "Synthwave Twilight",
    description: "Deep twilight cosmic violet with electric neon magenta and purple highlights.",
    category: "retro",
    colors: {
      panel: "#0f0b18",
      panelFace: "#181226",
      panelRecess: "#090610",
      panelSunk: "#050309",
      edge: "#2e214a",
      edgeStrong: "#4d377a",
      ink: "#f5f3ff",
      inkDim: "#c4b5fd",
      inkFaint: "#7c6da8",
      route: "#a855f7",
      clear: "#06b6d4",
      caution: "#f59e0b",
      danger: "#ec4899",
      paused: "#f97316",
      onSignal: "#090610",
      lampOff: "#241a38",
      brass: "#c084fc",
      blockClear: "#241a38",
    },
  },
  {
    id: "high-contrast-ops",
    name: "High-Contrast Ops",
    description: "Pure pitch black with maximized luminance edge rules and saturated signal clarity.",
    category: "contrast",
    colors: {
      panel: "#000000",
      panelFace: "#111111",
      panelRecess: "#000000",
      panelSunk: "#000000",
      edge: "#4f4f4f",
      edgeStrong: "#808080",
      ink: "#ffffff",
      inkDim: "#d0d0d0",
      inkFaint: "#a8a8a8",
      route: "#3b82f6",
      clear: "#22c55e",
      caution: "#eab308",
      danger: "#ef4444",
      paused: "#ff8800",
      onSignal: "#000000",
      lampOff: "#333333",
      brass: "#fbbf24",
      blockClear: "#333333",
    },
  },
];

const STORAGE_KEY = "zoltraak.appearance.v1";
const LEGACY_THEME_KEY = "zoltraak.theme";

export const DEFAULT_APPEARANCE: AppearanceSettings = {
  syncAppIcon: true,
  iconPalettes: {},
  syncExtensionTheme: true,
  themeMode: "system",
  activePresetId: "classic-dispatch",
  customColors: { ...DEFAULT_DARK_COLORS },
  userPresets: [],
  uiScale: 100,
  density: "default",
  radius: "bevel",
  fontMode: "default",
  customFont: null,
  effects: {
    scanlines: false,
    scanlinesIntensity: 70,
    grid: false,
    gridIntensity: 50,
    glass: false,
    glassIntensity: 60,
    glassStyle: "liquid",
    glassThemeOverride: true,
    glassNavbar: true,
    glassModals: true,
    glassSettings: true,
    glassOverlays: true,
    vignette: false,
    vignetteIntensity: 50,
    bloom: false,
    bloomIntensity: 50,
    noise: false,
    noiseIntensity: 35,
    noiseGrainSize: 1,
    noisePlacement: "overlay",
    warmth: false,
    warmthIntensity: 40,
  },
  motion: "full",
  fileOpenFeedback: true,
  fileOpenFeedbackDuration: 500,
  downloadDoubleClickInterval: 250,
  hardwareAcceleration: true,
  webGpu: true,
  telemetry: {
    dateFormat: "relative",
    speedUnit: "MBps",
  },
  columns: [...DEFAULT_COLUMNS],
  confettiOnComplete: false,
  confettiSfx: false,
  notificationLimit: 25,
};

export const COLOR_VAR_MAP: Record<keyof CustomColors, string> = {
  panel: "--c-panel",
  panelFace: "--c-panel-face",
  panelRecess: "--c-panel-recess",
  panelSunk: "--c-panel-sunk",
  edge: "--c-edge",
  edgeStrong: "--c-edge-strong",
  ink: "--c-ink",
  inkDim: "--c-ink-dim",
  inkFaint: "--c-ink-faint",
  route: "--c-route",
  clear: "--c-clear",
  caution: "--c-caution",
  danger: "--c-danger",
  paused: "--c-paused",
  onSignal: "--c-on-signal",
  lampOff: "--c-lamp-off",
  brass: "--c-brass",
  blockClear: "--c-block-clear",
};

/**
 * The column set a stored settings blob asks for.
 *
 * A blob written before columns existed carries four booleans instead, and the
 * user turned those off on purpose — so they are translated rather than
 * discarded, and only the columns they never had a switch for come in from the
 * defaults.
 */
function columnsFrom(parsed: Partial<AppearanceSettings> & { telemetry?: LegacyTelemetryOptions }): ColumnId[] {
  if (parsed.columns !== undefined) return normalizeColumns(parsed.columns);

  const legacy = parsed.telemetry;
  if (!legacy) return [...DEFAULT_COLUMNS];

  const dropped = new Set<ColumnId>();
  if (legacy.showOccupancy === false) dropped.add("occupancy");
  if (legacy.showSpeedBadge === false) dropped.add("rate");
  if (legacy.showEta === false) dropped.add("eta");
  // The connection count was a badge rather than a column, and it is not in
  // the default set, so its "on" is what carries information here.
  const columns = DEFAULT_COLUMNS.filter((id) => !dropped.has(id));
  return normalizeColumns(
    legacy.showConnections ? [...columns, "connections" as ColumnId] : columns,
  );
}

export function readAppearance(): AppearanceSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<AppearanceSettings> & {
        telemetry?: TelemetryOptions & LegacyTelemetryOptions;
      };
      return {
        ...DEFAULT_APPEARANCE,
        ...parsed,
        syncAppIcon: parsed.syncAppIcon !== false,
        iconPalettes: Object.fromEntries(Object.entries(parsed.iconPalettes && typeof parsed.iconPalettes === "object" ? parsed.iconPalettes : {}).flatMap(([key, value]) => {
          const palette = normalizeIconPalette(value);
          return palette ? [[key, palette]] : [];
        })),
        customColors: {
          ...DEFAULT_APPEARANCE.customColors,
          ...(parsed.customColors || {}),
        },
        effects: {
          ...DEFAULT_APPEARANCE.effects,
          ...(parsed.effects || {}),
          scanlines: Boolean(parsed.effects?.scanlines),
          scanlinesIntensity:
            typeof parsed.effects?.scanlinesIntensity === "number"
              ? Math.max(10, Math.min(100, Math.round(parsed.effects.scanlinesIntensity)))
              : DEFAULT_APPEARANCE.effects.scanlinesIntensity,
          grid: Boolean(parsed.effects?.grid),
          gridIntensity:
            typeof parsed.effects?.gridIntensity === "number"
              ? Math.max(10, Math.min(100, Math.round(parsed.effects.gridIntensity)))
              : DEFAULT_APPEARANCE.effects.gridIntensity,
          glass: Boolean(parsed.effects?.glass),
          glassIntensity:
            typeof parsed.effects?.glassIntensity === "number"
              ? Math.max(10, Math.min(100, Math.round(parsed.effects.glassIntensity)))
              : DEFAULT_APPEARANCE.effects.glassIntensity,
          glassStyle: parsed.effects?.glassStyle === "frosted" ? "frosted" : "liquid",
          glassThemeOverride: parsed.effects?.glassThemeOverride !== false,
          glassNavbar: parsed.effects?.glassNavbar !== false,
          glassModals: parsed.effects?.glassModals !== false,
          glassSettings: parsed.effects?.glassSettings !== false,
          glassOverlays: parsed.effects?.glassOverlays !== false,
          vignette: Boolean(parsed.effects?.vignette),
          vignetteIntensity:
            typeof parsed.effects?.vignetteIntensity === "number"
              ? Math.max(10, Math.min(100, Math.round(parsed.effects.vignetteIntensity)))
              : DEFAULT_APPEARANCE.effects.vignetteIntensity,
          bloom: Boolean(parsed.effects?.bloom),
          bloomIntensity:
            typeof parsed.effects?.bloomIntensity === "number"
              ? Math.max(10, Math.min(100, Math.round(parsed.effects.bloomIntensity)))
              : DEFAULT_APPEARANCE.effects.bloomIntensity,
          noise: Boolean(parsed.effects?.noise),
          noiseIntensity:
            typeof parsed.effects?.noiseIntensity === "number"
              ? Math.max(10, Math.min(100, Math.round(parsed.effects.noiseIntensity)))
              : DEFAULT_APPEARANCE.effects.noiseIntensity,
          noisePlacement: parsed.effects?.noisePlacement === "underlay" ? "underlay" : "overlay",
          noiseGrainSize: typeof parsed.effects?.noiseGrainSize === "number" && Number.isFinite(parsed.effects.noiseGrainSize)
            ? Math.max(0.5, Math.min(4, Math.round(parsed.effects.noiseGrainSize * 4) / 4))
            : DEFAULT_APPEARANCE.effects.noiseGrainSize,
          warmth: Boolean(parsed.effects?.warmth),
          warmthIntensity:
            typeof parsed.effects?.warmthIntensity === "number"
              ? Math.max(10, Math.min(100, Math.round(parsed.effects.warmthIntensity)))
              : DEFAULT_APPEARANCE.effects.warmthIntensity,
        },
        telemetry: {
          ...DEFAULT_APPEARANCE.telemetry,
          dateFormat: parsed.telemetry?.dateFormat ?? DEFAULT_APPEARANCE.telemetry.dateFormat,
          speedUnit: (parsed.telemetry as TelemetryOptions | undefined)?.speedUnit ?? DEFAULT_APPEARANCE.telemetry.speedUnit,
        },
        hardwareAcceleration:
          parsed.hardwareAcceleration !== undefined
            ? Boolean(parsed.hardwareAcceleration)
            : DEFAULT_APPEARANCE.hardwareAcceleration,
        webGpu: parsed.webGpu !== undefined ? Boolean(parsed.webGpu) : DEFAULT_APPEARANCE.webGpu,
        columns: columnsFrom(parsed),
        userPresets: Array.isArray(parsed.userPresets) ? parsed.userPresets : [],
        motion: parsed.motion === "subtle" || parsed.motion === "reduced" ? parsed.motion : "full",
        customFont: normalizeCustomFont(parsed.customFont),
        fontMode: parsed.fontMode === "custom" ? (normalizeCustomFont(parsed.customFont) ? "custom" : "default")
          : PRESET_FONT_MODES.includes(parsed.fontMode as FontMode) ? parsed.fontMode as FontMode : DEFAULT_APPEARANCE.fontMode,
        fileOpenFeedback: typeof parsed.fileOpenFeedback === "boolean" ? parsed.fileOpenFeedback : DEFAULT_APPEARANCE.fileOpenFeedback,
        fileOpenFeedbackDuration: typeof parsed.fileOpenFeedbackDuration === "number" && Number.isFinite(parsed.fileOpenFeedbackDuration)
          ? Math.max(100, Math.min(600, Math.round(parsed.fileOpenFeedbackDuration)))
          : DEFAULT_APPEARANCE.fileOpenFeedbackDuration,
        downloadDoubleClickInterval: typeof parsed.downloadDoubleClickInterval === "number" && Number.isFinite(parsed.downloadDoubleClickInterval)
          ? Math.max(100, Math.min(600, Math.round(parsed.downloadDoubleClickInterval)))
          : DEFAULT_APPEARANCE.downloadDoubleClickInterval,
        confettiOnComplete:
          parsed.confettiOnComplete !== undefined
            ? Boolean(parsed.confettiOnComplete)
            : DEFAULT_APPEARANCE.confettiOnComplete,
        confettiSfx:
          parsed.confettiSfx !== undefined
            ? Boolean(parsed.confettiSfx)
            : DEFAULT_APPEARANCE.confettiSfx,
        notificationLimit:
          typeof parsed.notificationLimit === "number" && !isNaN(parsed.notificationLimit)
            ? Math.max(1, Math.min(200, Math.round(parsed.notificationLimit)))
            : DEFAULT_APPEARANCE.notificationLimit,
      };
    }
  } catch {
    // Storage read error fallback
  }

  // Fallback to legacy theme key if present
  try {
    const legacy = localStorage.getItem(LEGACY_THEME_KEY);
    if (legacy === "light" || legacy === "dark" || legacy === "system") {
      return {
        ...DEFAULT_APPEARANCE,
        themeMode: legacy,
      };
    }
  } catch {
    // Legacy fallback ignored
  }

  return DEFAULT_APPEARANCE;
}

export function resolvedTheme(settings: AppearanceSettings, systemDark: boolean) {
  const isGlassThemeOverride = Boolean(settings.effects?.glass && settings.effects?.glassThemeOverride !== false);
  const colors = isGlassThemeOverride
    ? LIQUID_GLASS_COLORS
    : settings.themeMode === "custom"
      ? settings.customColors
      : settings.themeMode === "preset"
        ? settings.userPresets.find((p) => p.id === settings.activePresetId)?.colors
          ?? BUILTIN_PRESETS.find((p) => p.id === settings.activePresetId)?.colors ?? DEFAULT_DARK_COLORS
        : settings.themeMode === "light" || (settings.themeMode === "system" && !systemDark)
          ? DEFAULT_LIGHT_COLORS : DEFAULT_DARK_COLORS;
  const variables = Object.fromEntries(Object.entries(COLOR_VAR_MAP).map(([key, css]) => [
    css.replace("--c-", "--").replace("--panel-face", "--face").replace("--panel-recess", "--recess"),
    colors[key as keyof CustomColors],
  ]));
  const rgb = colors.panel.match(/[a-f\d]{2}/gi)?.map((v) => parseInt(v, 16)) ?? [0, 0, 0];
  const colorScheme = (rgb[0] ?? 0) * .299 + (rgb[1] ?? 0) * .587 + (rgb[2] ?? 0) * .114 > 145 ? "light" : "dark";
  return { colors, variables, colorScheme };
}

export function extensionTheme(settings: AppearanceSettings, systemDark: boolean) {
  return settings.syncExtensionTheme ? resolvedTheme(settings, systemDark) : null;
}

/** System light/dark share the corresponding explicit theme's icon palette. */
export function resolvedIconPalette(settings: AppearanceSettings, systemDark: boolean) {
  const { colors } = resolvedTheme(settings, systemDark);
  const key = settings.effects.glass && settings.effects.glassThemeOverride !== false ? "liquid-glass"
    : settings.themeMode === "preset" ? `preset:${settings.activePresetId}`
    : settings.themeMode === "system" ? (systemDark ? "dark" : "light") : settings.themeMode;
  const saved = normalizeIconPalette(settings.iconPalettes[key])
    ?? (settings.themeMode === "preset" && key.startsWith("preset:") ? normalizeIconPalette(settings.userPresets.find(p => p.id === settings.activePresetId)?.iconPalette) : undefined);
  const palette = saved ?? { useAppColors: true, foreground: colors.route };
  const foreground = palette.useAppColors ? colors.route : palette.foreground;
  return { key, ...palette, foreground: /^#[a-f\d]{6}$/i.test(foreground) ? foreground : DEFAULT_DARK_COLORS.route };
}

export function writeAppearance(settings: AppearanceSettings) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
    // Keep legacy key synced for external readers
    if (settings.themeMode === "light" || settings.themeMode === "dark" || settings.themeMode === "system") {
      localStorage.setItem(LEGACY_THEME_KEY, settings.themeMode);
    } else {
      localStorage.setItem(LEGACY_THEME_KEY, "dark");
    }
  } catch {
    // LocalStorage quota or access error
  }
}

let saveTimer: ReturnType<typeof setTimeout> | null = null;
let pendingSaveSettings: AppearanceSettings | null = null;

export function flushWriteAppearance() {
  if (saveTimer !== null) {
    clearTimeout(saveTimer);
    saveTimer = null;
  }
  if (pendingSaveSettings !== null) {
    const s = pendingSaveSettings;
    pendingSaveSettings = null;
    writeAppearance(s);
  }
}

export function scheduleWriteAppearance(settings: AppearanceSettings) {
  pendingSaveSettings = settings;
  if (saveTimer !== null) {
    clearTimeout(saveTimer);
  }
  saveTimer = setTimeout(() => {
    flushWriteAppearance();
  }, 350);
}

if (typeof window !== "undefined") {
  window.addEventListener("beforeunload", flushWriteAppearance);
  window.addEventListener("pagehide", flushWriteAppearance);
}

function clearCustomColorVariables(root: HTMLElement) {
  for (const varName of Object.values(COLOR_VAR_MAP)) {
    root.style.removeProperty(varName);
  }
}

function applyCustomColorVariables(root: HTMLElement, colors: CustomColors) {
  for (const [key, varName] of Object.entries(COLOR_VAR_MAP) as [keyof CustomColors, string][]) {
    const val = colors[key];
    if (val && root.style.getPropertyValue(varName) !== val) {
      root.style.setProperty(varName, val);
    }
  }
}

let noiseDataUrlCache: string | null = null;
export function getNoisePatternDataUrl(): string {
  if (typeof document === "undefined") return "";
  if (noiseDataUrlCache) return noiseDataUrlCache;
  try {
    const canvas = document.createElement("canvas");
    canvas.width = 128;
    canvas.height = 128;
    const ctx = canvas.getContext("2d");
    if (!ctx) return "";
    const imgData = ctx.createImageData(128, 128);
    const data = imgData.data;
    for (let i = 0; i < data.length; i += 4) {
      const v = Math.floor(Math.random() * 255);
      data[i] = v;
      data[i + 1] = v;
      data[i + 2] = v;
      data[i + 3] = Math.floor(Math.random() * 160 + 60);
    }
    ctx.putImageData(imgData, 0, 0);
    noiseDataUrlCache = canvas.toDataURL("image/png");
    return noiseDataUrlCache;
  } catch {
    return "";
  }
}

let lastWindowRadius = "";

/** The frameless window's own corners follow the Corner Radius setting too. */
function syncWindowCorners(radius: string) {
  if (radius === lastWindowRadius || !("__TAURI_INTERNALS__" in window)) return;
  lastWindowRadius = radius;
  void import("@tauri-apps/api/core")
    .then(({ invoke }) => invoke("set_window_corners", { radius }))
    .catch(() => { lastWindowRadius = ""; });
}

export function applyAppearance(settings: AppearanceSettings) {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  const body = document.body;

  const isGlassThemeOverride = Boolean(settings.effects?.glass && settings.effects?.glassThemeOverride !== false);

  // 1. Theme and color variables
  if (isGlassThemeOverride) {
    root.setAttribute("data-theme", "liquid-glass");
    applyCustomColorVariables(root, LIQUID_GLASS_COLORS);
  } else if (settings.themeMode === "system") {
    root.removeAttribute("data-theme");
    clearCustomColorVariables(root);
  } else if (settings.themeMode === "light") {
    root.setAttribute("data-theme", "light");
    clearCustomColorVariables(root);
  } else if (settings.themeMode === "dark") {
    root.setAttribute("data-theme", "dark");
    clearCustomColorVariables(root);
  } else if (settings.themeMode === "preset") {
    root.setAttribute("data-theme", "preset");
    const userPreset = settings.userPresets.find((p) => p.id === settings.activePresetId);
    const builtinPreset = BUILTIN_PRESETS.find((p) => p.id === settings.activePresetId);
    const colors = userPreset?.colors || builtinPreset?.colors || DEFAULT_DARK_COLORS;
    applyCustomColorVariables(root, colors);
  } else if (settings.themeMode === "custom") {
    root.setAttribute("data-theme", "custom");
    applyCustomColorVariables(root, settings.customColors);
  }

  // 2. UI Scale
  const clampedScale = Math.max(80, Math.min(130, settings.uiScale || 100));
  root.style.setProperty("--ui-scale", `${clampedScale}%`);
  // Use CSS zoom for crisp webview scaling
  (root.style as CSSStyleDeclaration & { zoom?: string }).zoom = `${clampedScale / 100}`;

  // 3. Layout Density & Geometry
  root.setAttribute("data-density", settings.density || "default");
  root.setAttribute("data-radius", settings.radius || "bevel");
  syncWindowCorners(settings.radius || "bevel");
  const fontMode = settings.fontMode === "custom" && !settings.customFont ? "default" : settings.fontMode || "default";
  root.setAttribute("data-font", fontMode);
  // An uploaded face is loaded from storage and wins over the preset variables;
  // any other mode (or a failed load) falls back to the presets in the stylesheet.
  void applyCustomFont(root, fontMode === "custom" ? settings.customFont : null, {
    sans: '"Archivo Variable", ui-sans-serif, system-ui, sans-serif',
    mono: '"Martian Mono Variable", ui-monospace, monospace',
  });
  root.setAttribute("data-motion", settings.motion || "full");
  root.setAttribute("data-hardware-acceleration", String(settings.hardwareAcceleration !== false));
  root.setAttribute("data-webgpu", String(settings.webGpu !== false));

  // 4. Visual effects
  if (body) {
    body.setAttribute("data-effect-scanlines", String(Boolean(settings.effects?.scanlines)));
    body.setAttribute("data-effect-grid", String(Boolean(settings.effects?.grid)));
    body.setAttribute("data-effect-glass", String(Boolean(settings.effects?.glass)));
    body.setAttribute("data-effect-vignette", String(Boolean(settings.effects?.vignette)));
    body.setAttribute("data-effect-bloom", String(Boolean(settings.effects?.bloom)));
    body.setAttribute("data-effect-noise", String(Boolean(settings.effects?.noise)));
    body.setAttribute("data-effect-warmth", String(Boolean(settings.effects?.warmth)));

    const fx = settings.effects || DEFAULT_APPEARANCE.effects;
    body.setAttribute("data-glass-style", fx.glassStyle || "liquid");
    body.setAttribute("data-glass-theme-override", String(fx.glassThemeOverride !== false));
    body.setAttribute("data-glass-navbar", String(fx.glassNavbar !== false));
    body.setAttribute("data-glass-modals", String(fx.glassModals !== false));
    body.setAttribute("data-glass-settings", String(fx.glassSettings !== false));
    body.setAttribute("data-glass-overlays", String(fx.glassOverlays !== false));
    body.setAttribute("data-noise-placement", fx.noisePlacement || "overlay");

    const scanlinesVal = fx.scanlinesIntensity ?? 70;
    const gridVal = fx.gridIntensity ?? 50;
    const glassVal = fx.glassIntensity ?? 60;
    const vignetteVal = fx.vignetteIntensity ?? 50;
    const bloomVal = fx.bloomIntensity ?? 50;
    const noiseVal = fx.noiseIntensity ?? 35;
    root.style.setProperty("--effect-noise-size", `${128 * (fx.noiseGrainSize ?? 1)}px`);
    const warmthVal = fx.warmthIntensity ?? 40;

    root.style.setProperty("--effect-scanlines-opacity", (scanlinesVal / 100).toFixed(2));
    root.style.setProperty("--effect-grid-opacity", (gridVal / 100).toFixed(2));
    root.style.setProperty("--effect-glass-blur", `${Math.round(8 + glassVal * 0.28)}px`);
    root.style.setProperty("--effect-glass-saturation", `${Math.round(130 + glassVal * 0.7)}%`);
    root.style.setProperty("--effect-vignette-opacity", (vignetteVal / 100).toFixed(2));
    root.style.setProperty("--effect-bloom-intensity", (bloomVal / 100).toFixed(2));
    root.style.setProperty("--effect-noise-opacity", ((noiseVal / 100) * 0.32).toFixed(3));
    root.style.setProperty("--effect-warmth-opacity", (warmthVal / 100).toFixed(2));

    const pattern = getNoisePatternDataUrl();
    if (pattern) {
      root.style.setProperty("--effect-noise-pattern", `url("${pattern}")`);
    }
  }
}

/** Pre-paint bootstrap function. Must be called before React render. */
export function bootstrapAppearance() {
  applyAppearance(readAppearance());
}

// Event-based subscription for multi-component reactivity without provider boilerplate
const listeners = new Set<() => void>();
let cachedSettings = readAppearance();
let emitRaf: number | null = null;

function emitChange() {
  for (const listener of listeners) {
    listener();
  }
}

function scheduleEmitChange() {
  if (emitRaf !== null) return;
  if (typeof requestAnimationFrame !== "undefined") {
    emitRaf = requestAnimationFrame(() => {
      emitRaf = null;
      emitChange();
    });
  } else {
    emitChange();
  }
}

export function useAppearance() {
  const settings = useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => cachedSettings,
  );

  const update = useCallback((updater: Partial<AppearanceSettings> | ((prev: AppearanceSettings) => AppearanceSettings)) => {
    const next = typeof updater === "function" ? updater(cachedSettings) : { ...cachedSettings, ...updater };
    cachedSettings = next;
    flushWriteAppearance();
    writeAppearance(next);
    applyAppearance(next);
    emitChange();
  }, []);

  const setThemeMode = useCallback((mode: ThemeMode) => {
    update({ themeMode: mode });
  }, [update]);

  const selectPreset = useCallback((presetId: string) => {
    const builtin = BUILTIN_PRESETS.find((p) => p.id === presetId);
    const userPreset = cachedSettings.userPresets.find((p) => p.id === presetId);
    const colors = userPreset?.colors || builtin?.colors;

    update({
      themeMode: "preset",
      activePresetId: presetId,
      ...(colors ? { customColors: { ...colors } } : {}),
    });
  }, [update]);

  const setCustomColor = useCallback((key: keyof CustomColors, value: string) => {
    // 1. Instant zero-latency native GPU update via CSS variable
    const varName = COLOR_VAR_MAP[key];
    if (varName && typeof document !== "undefined") {
      const root = document.documentElement;
      if (root.style.getPropertyValue(varName) !== value) {
        root.style.setProperty(varName, value);
      }
      if (root.getAttribute("data-theme") !== "custom") {
        root.setAttribute("data-theme", "custom");
      }
    }

    // 2. Immediate in-memory state update
    const nextColors = { ...cachedSettings.customColors, [key]: value };
    const next: AppearanceSettings = {
      ...cachedSettings,
      themeMode: "custom",
      customColors: nextColors,
    };
    cachedSettings = next;

    // 3. Debounced disk I/O (never blocks the drag frame)
    scheduleWriteAppearance(next);

    // 4. Coalesce React notifications to the browser animation frame
    scheduleEmitChange();
  }, []);

  const setAllCustomColors = useCallback((colors: CustomColors) => {
    update({
      themeMode: "custom",
      customColors: { ...colors },
    });
  }, [update]);

  const saveUserPreset = useCallback((name: string, description?: string, customColors?: CustomColors) => {
    const id = `user-${Date.now()}`;
    const systemDark = typeof matchMedia !== "undefined" && matchMedia("(prefers-color-scheme: dark)").matches;
    const colorsToSave = customColors || resolvedTheme(cachedSettings, systemDark).colors;
    const { key: _, ...iconPalette } = resolvedIconPalette(cachedSettings, systemDark);
    const newPreset: UserPreset = {
      id,
      name: name.trim() || "Custom Theme",
      description: description?.trim() || "User-authored custom color theme",
      colors: { ...colorsToSave },
      createdAt: Date.now(),
      iconPalette,
    };
    update((prev) => ({
      ...prev,
      activePresetId: id,
      themeMode: "preset",
      customColors: { ...colorsToSave },
      userPresets: [newPreset, ...prev.userPresets],
    }));
    return id;
  }, [update]);

  const deleteUserPreset = useCallback((id: string) => {
    update((prev) => ({
      ...prev,
      activePresetId: prev.activePresetId === id ? "classic-dispatch" : prev.activePresetId,
      themeMode: prev.activePresetId === id ? "dark" : prev.themeMode,
      userPresets: prev.userPresets.filter((p) => p.id !== id),
    }));
  }, [update]);

  const setUiScale = useCallback((scale: number) => {
    update({ uiScale: Math.max(80, Math.min(130, scale)) });
  }, [update]);

  const setDensity = useCallback((density: Density) => {
    update({ density });
  }, [update]);

  const setRadius = useCallback((radius: CornerRadius) => {
    update({ radius });
  }, [update]);

  const setFontMode = useCallback((fontMode: FontMode) => {
    update({ fontMode });
  }, [update]);

  /** Keep (or, with null, drop) the uploaded typeface; keeping it also selects it. */
  const setCustomFont = useCallback((customFont: CustomFontMeta | null) => {
    update((prev) => ({
      ...prev,
      customFont,
      fontMode: customFont ? "custom" : prev.fontMode === "custom" ? "default" : prev.fontMode,
    }));
  }, [update]);

  const setMotion = useCallback((motion: MotionMode) => {
    update({ motion });
  }, [update]);

  const setAnimationsEnabled = useCallback((enabled: boolean) => {
    update({ motion: enabled ? "full" : "reduced" });
  }, [update]);

  const setConfettiOnComplete = useCallback((confettiOnComplete: boolean) => {
    update({ confettiOnComplete });
  }, [update]);

  const setConfettiSfx = useCallback((confettiSfx: boolean) => {
    update({ confettiSfx });
  }, [update]);

  const setNotificationLimit = useCallback((limit: number) => {
    const clamped = Math.max(1, Math.min(200, Math.round(limit || 25)));
    update({ notificationLimit: clamped });
  }, [update]);

  const setHardwareAcceleration = useCallback((hardwareAcceleration: boolean) => {
    update({ hardwareAcceleration });
  }, [update]);

  const setWebGpu = useCallback((webGpu: boolean) => {
    update({ webGpu });
  }, [update]);

  const toggleEffect = useCallback((key: keyof AtmosphericEffects) => {
    update((prev) => ({
      ...prev,
      effects: {
        ...prev.effects,
        [key]: !prev.effects[key],
      },
    }));
  }, [update]);

  const setEffect = useCallback((key: keyof AtmosphericEffects, enabled: boolean) => {
    update((prev) => ({
      ...prev,
      effects: {
        ...prev.effects,
        [key]: enabled,
      },
    }));
  }, [update]);

  const setEffectIntensity = useCallback((key: keyof AtmosphericEffects, value: number) => {
    const clamped = Math.max(10, Math.min(100, Math.round(value)));

    // Fast zero-latency CSS custom property update during slider scrub
    if (typeof document !== "undefined") {
      const root = document.documentElement;
      if (key === "scanlinesIntensity") {
        root.style.setProperty("--effect-scanlines-opacity", (clamped / 100).toFixed(2));
      } else if (key === "gridIntensity") {
        root.style.setProperty("--effect-grid-opacity", (clamped / 100).toFixed(2));
      } else if (key === "glassIntensity") {
        root.style.setProperty("--effect-glass-blur", `${Math.round(8 + clamped * 0.28)}px`);
        root.style.setProperty("--effect-glass-saturation", `${Math.round(130 + clamped * 0.7)}%`);
      } else if (key === "vignetteIntensity") {
        root.style.setProperty("--effect-vignette-opacity", (clamped / 100).toFixed(2));
      } else if (key === "bloomIntensity") {
        root.style.setProperty("--effect-bloom-intensity", (clamped / 100).toFixed(2));
      } else if (key === "noiseIntensity") {
        root.style.setProperty("--effect-noise-opacity", ((clamped / 100) * 0.32).toFixed(3));
        const pattern = getNoisePatternDataUrl();
        if (pattern) {
          root.style.setProperty("--effect-noise-pattern", `url("${pattern}")`);
        }
      } else if (key === "warmthIntensity") {
        root.style.setProperty("--effect-warmth-opacity", (clamped / 100).toFixed(2));
      }
    }

    const nextEffects = { ...cachedSettings.effects, [key]: clamped };
    const next: AppearanceSettings = {
      ...cachedSettings,
      effects: nextEffects,
    };
    cachedSettings = next;
    scheduleWriteAppearance(next);
    scheduleEmitChange();
  }, []);

  const setGlassStyle = useCallback((style: "liquid" | "frosted") => {
    update((prev) => ({
      ...prev,
      effects: {
        ...prev.effects,
        glassStyle: style,
      },
    }));
  }, [update]);

  const setNoisePlacement = useCallback((placement: "underlay" | "overlay") => {
    update((prev) => ({
      ...prev,
      effects: {
        ...prev.effects,
        noisePlacement: placement,
      },
    }));
  }, [update]);

  const toggleTelemetry = useCallback((key: keyof TelemetryOptions) => {
    if (key === "dateFormat") {
      update((prev) => ({
        ...prev,
        telemetry: {
          ...prev.telemetry,
          dateFormat: prev.telemetry.dateFormat === "relative" ? "absolute" : "relative",
        },
      }));
    } else if (key === "speedUnit") {
      update((prev) => ({
        ...prev,
        telemetry: {
          ...prev.telemetry,
          speedUnit: prev.telemetry.speedUnit === "MBps" ? "Mbps" : "MBps",
        },
      }));
    }
  }, [update]);

  const toggleColumn = useCallback((id: ColumnId) => {
    update((prev) => {
      const visible = new Set(prev.columns);
      if (visible.has(id)) visible.delete(id);
      else visible.add(id);
      return { ...prev, columns: normalizeColumns(Array.from(visible)) };
    });
  }, [update]);

  const setColumns = useCallback((ids: ColumnId[]) => {
    update((prev) => ({ ...prev, columns: normalizeColumns(ids) }));
  }, [update]);

  const resetColumns = useCallback(() => {
    update((prev) => ({ ...prev, columns: [...DEFAULT_COLUMNS] }));
  }, [update]);

  const resetDefaults = useCallback(() => {
    void deleteCustomFontData().catch(() => undefined);
    cachedSettings = { ...DEFAULT_APPEARANCE };
    writeAppearance(DEFAULT_APPEARANCE);
    applyAppearance(DEFAULT_APPEARANCE);
    emitChange();
  }, []);

  const exportThemeJson = useCallback(() => {
    const systemDark = typeof matchMedia !== "undefined" && matchMedia("(prefers-color-scheme: dark)").matches;
    const { key: _, ...iconPalette } = resolvedIconPalette(cachedSettings, systemDark);
    const exportData = {
      version: 1,
      appName: "Zoltraak",
      type: "theme-export",
      themeMode: cachedSettings.themeMode,
      activePresetId: cachedSettings.activePresetId,
      colors: resolvedTheme(cachedSettings, systemDark).colors,
      density: cachedSettings.density,
      radius: cachedSettings.radius,
      // An uploaded font is a file on this machine, so it does not travel.
      fontMode: cachedSettings.fontMode === "custom" ? "default" : cachedSettings.fontMode,
      motion: cachedSettings.motion,
      hardwareAcceleration: cachedSettings.hardwareAcceleration,
      webGpu: cachedSettings.webGpu,
      effects: cachedSettings.effects,
      iconPalette,
    };
    return JSON.stringify(exportData, null, 2);
  }, []);

  const importThemeJson = useCallback((jsonString: string): { ok: boolean; error?: string } => {
    try {
      const data = JSON.parse(jsonString) as {
        colors?: Partial<CustomColors>;
        density?: Density;
        radius?: CornerRadius;
        fontMode?: FontMode;
        motion?: MotionMode;
        hardwareAcceleration?: boolean;
        webGpu?: boolean;
        effects?: Partial<AtmosphericEffects>;
        iconPalette?: IconPalette;
      };
      if (!data || typeof data !== "object") {
        return { ok: false, error: "Invalid JSON structure." };
      }
      if (!data.colors || typeof data.colors !== "object") {
        return { ok: false, error: "Theme JSON must contain a valid 'colors' object." };
      }
      if (data.iconPalette !== undefined && !normalizeIconPalette(data.iconPalette)) return { ok: false, error: "Icon palette must contain valid six-digit hex colors." };

      update((prev) => {
        const next: AppearanceSettings = {
        ...prev,
        themeMode: "custom",
        customColors: {
          ...prev.customColors,
          ...data.colors,
        },
        ...(data.density ? { density: data.density } : {}),
        ...(data.radius ? { radius: data.radius } : {}),
        ...(data.fontMode && PRESET_FONT_MODES.includes(data.fontMode) && data.fontMode !== "custom" ? { fontMode: data.fontMode } : {}),
        ...(data.motion ? { motion: data.motion } : {}),
        ...(data.hardwareAcceleration !== undefined ? { hardwareAcceleration: Boolean(data.hardwareAcceleration) } : {}),
        ...(data.webGpu !== undefined ? { webGpu: Boolean(data.webGpu) } : {}),
        ...(data.effects ? { effects: { ...prev.effects, ...data.effects } } : {}),
        };
        const systemDark = typeof matchMedia !== "undefined" && matchMedia("(prefers-color-scheme: dark)").matches;
        const target = resolvedIconPalette(next, systemDark);
        return { ...next, iconPalettes: { ...prev.iconPalettes, [target.key]: normalizeIconPalette(data.iconPalette) ?? { useAppColors: true, foreground: next.customColors.route } } };
      });

      return { ok: true };
    } catch (e) {
      return { ok: false, error: e instanceof Error ? e.message : "Failed to parse JSON." };
    }
  }, [update]);

  useEffect(() => {
    // Keep DOM in sync
    applyAppearance(settings);
  }, [settings]);

  return {
    settings,
    update,
    setThemeMode,
    selectPreset,
    setCustomColor,
    setAllCustomColors,
    saveUserPreset,
    deleteUserPreset,
    setUiScale,
    setDensity,
    setRadius,
    setFontMode,
    setCustomFont,
    setMotion,
    setAnimationsEnabled,
    animationsEnabled: settings.motion !== "reduced",
    confettiOnComplete: Boolean(settings.confettiOnComplete),
    setConfettiOnComplete,
    confettiSfx: Boolean(settings.confettiSfx),
    setConfettiSfx,
    notificationLimit: settings.notificationLimit || 25,
    setNotificationLimit,
    setHardwareAcceleration,
    setWebGpu,
    toggleEffect,
    setEffect,
    setEffectIntensity,
    setGlassStyle,
    setNoisePlacement,
    toggleTelemetry,
    toggleColumn,
    setColumns,
    resetColumns,
    resetDefaults,
    exportThemeJson,
    importThemeJson,
  };
}
