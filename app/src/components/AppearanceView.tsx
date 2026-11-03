import { memo, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { SlideGroup } from "./SlideGroup";
import { COLUMNS, DEFAULT_COLUMNS } from "../lib/columns";
import { flashFileOpen } from "../lib/fileOpenFeedback";
import {
  BUILTIN_PRESETS,
  DEFAULT_DARK_COLORS,
  DEFAULT_LIGHT_COLORS,
  LIQUID_GLASS_COLORS,
  useAppearance,
  resolvedIconPalette,
  type CornerRadius,
  type CustomColors,
  type Density,
  type MotionMode,
  type ThemeMode,
  type ThemePreset,
} from "../lib/useAppearance";
import { Button, Field, NumberInput, Section, Switch, TextInput } from "./Controls";
import { TypefacePicker } from "./TypefacePicker";
import {
  ArrowUpRight,
  Check,
  Copy,
  FileUp,
  LayoutGrid,
  Monitor,
  Moon,
  Palette,
  Pause,
  Play,
  RotateCcw,
  Sun,
  Trash,
} from "./Icons";
import { Legend } from "./Panel";
import { GpuVisualizer } from "./GpuVisualizer";
import { AppIconSettings } from "./AppIconSettings";
import { triggerConfetti } from "../lib/confetti";

function ToggleRow({
  title,
  description,
  checked,
  onChange,
}: {
  title: string;
  description: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <div className="flex items-start justify-between gap-6 border-b border-edge py-3 last:border-b-0">
      <div className="min-w-0">
        <p className="text-[13px] font-medium text-ink">{title}</p>
        <p className="mt-0.5 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">{description}</p>
      </div>
      <Switch label={title} checked={checked} onChange={onChange} />
    </div>
  );
}

function AtmosphericEffectRow({
  title,
  badge,
  tag,
  description,
  checked,
  onToggle,
  intensity,
  onIntensityChange,
  intensityLabel = "Intensity",
  min = 10,
  max = 100,
  step = 5,
  action,
  controls,
  children,
}: {
  title: string;
  badge: string;
  tag?: ReactNode;
  description: string;
  checked: boolean;
  onToggle: (checked: boolean) => void;
  intensity: number;
  onIntensityChange: (val: number) => void;
  intensityLabel?: string;
  min?: number;
  max?: number;
  step?: number;
  action?: ReactNode;
  controls?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="border-b border-edge py-3.5 last:border-b-0">
      <div className="flex items-start justify-between gap-6">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-[13px] font-medium text-ink">{title}</p>
            <span className="rounded-hw border border-edge-strong bg-face/70 px-1.5 py-[1px] text-[11px] font-mono uppercase tracking-wider text-ink-dim">
              {badge}
            </span>
            {tag}
            {action}
          </div>
          <p className="mt-0.5 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">{description}</p>
        </div>
        <Switch label={title} checked={checked} onChange={onToggle} />
      </div>

      {/* Intensity Slider Row */}
      <div
        className={`mt-3 flex items-center gap-3 transition-opacity duration-150 ${
          checked ? "opacity-100" : "opacity-40 pointer-events-none"
        }`}
      >
        <span className="min-w-[110px] text-[11px] font-mono uppercase tracking-wider text-ink-faint">
          {intensityLabel}
        </span>
        <input
          type="range"
          min={min}
          max={max}
          step={step}
          value={intensity}
          disabled={!checked}
          onChange={(e) => onIntensityChange(Number(e.target.value))}
          className="accent-route h-1.5 flex-1 cursor-pointer rounded-hw bg-recess disabled:cursor-not-allowed"
          aria-label={`${title} ${intensityLabel}`}
        />
        <span className="min-w-[42px] text-right font-mono text-[11px] font-semibold text-ink">
          {intensity}%
        </span>
      </div>

      {controls}
      {checked && children}
    </div>
  );
}

const COLOR_SLOT_LABELS: { key: keyof CustomColors; label: string; group: string; hint: string }[] = [
  // Surfaces
  { key: "panel", label: "Panel Canvas", group: "Surfaces", hint: "Main background of the window" },
  { key: "panelFace", label: "Plate Face", group: "Surfaces", hint: "Machined cards and elevated surfaces" },
  { key: "panelRecess", label: "Recess Well", group: "Surfaces", hint: "Inputs, search fields, and recessed slots" },
  { key: "panelSunk", label: "Sunk Trough", group: "Surfaces", hint: "Deep well surfaces" },

  // Etched rules & hairlines
  { key: "edge", label: "Hairline Rule", group: "Boundaries", hint: "Etched dividing lines and border hairlines" },
  { key: "edgeStrong", label: "Strong Rule", group: "Boundaries", hint: "Outer panel frames and active borders" },

  // Lettering
  { key: "ink", label: "Primary Lettering", group: "Lettering", hint: "Main headline and content text" },
  { key: "inkDim", label: "Dimmed Lettering", group: "Lettering", hint: "Secondary descriptions and metadata" },
  { key: "inkFaint", label: "Faint Lettering", group: "Lettering", hint: "Placeholders and inactive legends" },

  // Signals & Accents
  { key: "route", label: "Route Highlight", group: "Signals & Lamps", hint: "Primary focus ring, active tabs, and accents" },
  { key: "clear", label: "Clear Signal", group: "Signals & Lamps", hint: "Finished downloads and completed states" },
  { key: "caution", label: "Caution Signal", group: "Signals & Lamps", hint: "Waiting, warnings, and queues" },
  { key: "danger", label: "Danger Signal", group: "Signals & Lamps", hint: "Errors, faults, and critical stops" },
  { key: "paused", label: "Paused Signal", group: "Signals & Lamps", hint: "Paused downloads, lamps, and occupancy blocks" },
  { key: "onSignal", label: "On-Signal Lettering", group: "Signals & Lamps", hint: "Text sitting over saturated signal fills" },
  { key: "lampOff", label: "Unlit Housing", group: "Signals & Lamps", hint: "Dark lamp cell when unlit" },
  { key: "brass", label: "Engraved Brass", group: "Signals & Lamps", hint: "Metallic badge and telemetry accents" },
  { key: "blockClear", label: "Block Clear Track", group: "Signals & Lamps", hint: "Empty occupancy block track background" },
];

/**
 * Isolated, high-performance color slot item.
 * Directly applies CSS variables on native drag events without lagging React rendering.
 */
const ColorSlotItem = memo(function ColorSlotItem({
  slotKey,
  label,
  hint,
  value,
  onChange,
}: {
  slotKey: keyof CustomColors;
  label: string;
  hint: string;
  value: string;
  onChange: (key: keyof CustomColors, value: string) => void;
}) {
  const [localColor, setLocalColor] = useState(value);
  const rafRef = useRef<number | null>(null);

  useEffect(() => {
    setLocalColor(value);
  }, [value]);

  const handleInput = (newVal: string) => {
    setLocalColor(newVal);

    // Throttle React state update to animation frame
    if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null;
      onChange(slotKey, newVal);
    });
  };

  const handleBlur = () => {
    onChange(slotKey, localColor);
  };

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between text-[11px]">
        <span className="text-ink font-medium">{label}</span>
        <span className="describer text-[11px] text-ink-dim uppercase">{localColor}</span>
      </div>
      <div className="flex items-center gap-2">
        <label className="relative flex size-7 shrink-0 cursor-pointer overflow-hidden rounded-hw border border-edge-strong shadow-inner transition-transform active:scale-95">
          <input
            type="color"
            value={localColor}
            onInput={(e) => handleInput((e.target as HTMLInputElement).value)}
            onChange={(e) => handleInput(e.target.value)}
            className="absolute -inset-2 size-12 cursor-pointer opacity-0"
          />
          <span style={{ backgroundColor: localColor }} className="size-full" />
        </label>
        <TextInput
          value={localColor}
          onChange={handleInput}
          onBlur={handleBlur}
          mono
          placeholder="#000000"
          className="h-7 text-[11px]"
        />
      </div>
      <span className="text-[11px] leading-tight text-ink-faint">{hint}</span>
    </div>
  );
});

/** Live interactive miniature preview board */
const LiveDispatchPreview = memo(function LiveDispatchPreview({
  colors,
  density,
  radius,
  hasUnsavedChanges,
  onSave,
}: {
  colors: CustomColors;
  density: Density;
  radius: CornerRadius;
  hasUnsavedChanges?: boolean;
  onSave?: () => void;
}) {
  const [isPlaying, setIsPlaying] = useState(true);
  const [toggleState, setToggleState] = useState(true);
  const previewRadius = radius === "sharp" ? 0 : radius === "modern" ? 6 : radius === "rounded" ? 10 : 2;

  const previewCssVars = useMemo(
    () =>
      ({
        "--c-panel": colors.panel,
        "--c-panel-face": colors.panelFace,
        "--c-panel-recess": colors.panelRecess,
        "--c-panel-sunk": colors.panelSunk,
        "--c-edge": colors.edge,
        "--c-edge-strong": colors.edgeStrong,
        "--c-ink": colors.ink,
        "--c-ink-dim": colors.inkDim,
        "--c-ink-faint": colors.inkFaint,
        "--c-route": colors.route,
        "--c-clear": colors.clear,
        "--c-caution": colors.caution,
        "--c-danger": colors.danger,
        "--c-paused": colors.paused,
        "--c-on-signal": colors.onSignal,
        "--c-lamp-off": colors.lampOff,
        "--c-brass": colors.brass,
        "--c-block-clear": colors.blockClear,
      }) as React.CSSProperties,
    [colors],
  );

  return (
    <div
      style={{
        ...previewCssVars,
        backgroundColor: colors.panel,
        color: colors.ink,
        borderColor: colors.edge,
        borderRadius: previewRadius,
      }}
      className="flex flex-col gap-3 border p-4 shadow-sm transition-colors"
    >
      <div className="flex items-center justify-between border-b pb-2" style={{ borderColor: colors.edge }}>
        <div className="flex items-center gap-2">
          <span
            style={{
              backgroundColor: isPlaying ? colors.clear : colors.paused,
              width: 8,
              height: 8,
            }}
            className="inline-block rounded-hw-sm ring-1 ring-black/20"
          />
          <span className="legend text-[11px]" style={{ color: isPlaying ? colors.inkDim : colors.paused }}>
            {isPlaying ? "CIRCUIT ACTIVE" : "CIRCUIT PAUSED"}
          </span>
        </div>
        <div className="flex items-center gap-2">
          {hasUnsavedChanges ? (
            <div className="flex items-center gap-1.5">
              <span
                className="legend rounded-hw border px-2 py-[1px] text-[11px] font-medium"
                style={{
                  backgroundColor: colors.panelFace,
                  color: colors.caution,
                  borderColor: colors.caution,
                }}
              >
                ● UNSAVED DRAFT
              </span>
              {onSave && (
                <button
                  type="button"
                  onClick={onSave}
                  style={{
                    backgroundColor: colors.route,
                    color: colors.onSignal,
                  }}
                  className="legend rounded-hw px-2 py-[2px] text-[11px] font-semibold shadow-sm transition-opacity hover:opacity-90 active:scale-95"
                >
                  Apply to App
                </button>
              )}
            </div>
          ) : (
            <span
              className="legend rounded-hw border px-2 py-[1px] text-[11px]"
              style={{
                backgroundColor: colors.panelFace,
                color: colors.clear,
                borderColor: colors.clear,
              }}
            >
              ✓ ACTIVE THEME
            </span>
          )}
          <span
            className="legend px-1.5 py-[1px] text-[11px]"
            style={{ backgroundColor: colors.route, color: colors.onSignal, borderRadius: Math.min(previewRadius, 4) }}
          >
            LIVE PREVIEW
          </span>
        </div>
      </div>

      {/* Simulated download row */}
      <div
        style={{
          backgroundColor: colors.panelFace,
          borderColor: colors.edge,
          borderRadius: previewRadius,
        }}
        className={`flex flex-col gap-2 border p-3 ${
          density === "compact" ? "py-2" : density === "comfortable" ? "py-4" : "py-3"
        }`}
      >
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="text-[13px] font-medium" style={{ color: colors.ink }}>
                windows-setup-x64.iso
              </span>
              <span
                className="describer text-[11px]"
                style={{ color: colors.inkDim }}
              >
                1.28 GB / 1.42 GB
              </span>
            </div>
            <div className="mt-1 flex items-center gap-3 text-[11px]" style={{ color: colors.inkDim }}>
              <span className="describer" style={{ color: colors.route }}>
                48.6 MB/s
              </span>
              <span>·</span>
              <span>ETA 3s</span>
              <span>·</span>
              <span style={{ color: colors.brass }}>16 Conn</span>
            </div>
          </div>

          <div className="flex items-center gap-1.5">
            <button
              type="button"
              onClick={() => setIsPlaying(!isPlaying)}
              style={{
                backgroundColor: colors.panelRecess,
                borderColor: colors.edgeStrong,
                color: colors.ink,
              }}
              className="focus-ring flex size-7 items-center justify-center rounded-hw border transition-colors hover:brightness-110"
              title="Toggle playback"
            >
              {isPlaying ? <Pause className="size-3.5" /> : <Play className="size-3.5" />}
            </button>
            <button
              type="button"
              style={{
                backgroundColor: colors.panelRecess,
                borderColor: colors.edge,
                color: colors.danger,
              }}
              className="focus-ring flex size-7 items-center justify-center rounded-hw border transition-colors hover:brightness-110"
              title="Remove row"
            >
              <Trash className="size-3.5" />
            </button>
          </div>
        </div>

        {/* Chunk occupancy track */}
        <div className="flex h-[8px] w-full gap-[2px]" style={{ backgroundColor: colors.panelRecess }}>
          {Array.from({ length: 18 }).map((_, i) => (
            <span
              key={i}
              className="flex-1 rounded-hw-sm"
              style={{
                backgroundColor: i < 14 ? (isPlaying ? colors.clear : colors.paused) : colors.blockClear,
              }}
            />
          ))}
        </div>
      </div>

      {/* Simulated interactive controls */}
      <div className="flex flex-wrap items-center justify-between gap-3 pt-1">
        <div className="flex items-center gap-2">
          <input
            type="text"
            readOnly
            value="https://cdn.example.org/payload.bin"
            style={{
              backgroundColor: colors.panelRecess,
              borderColor: colors.edge,
              color: colors.ink,
            }}
            className="describer w-56 rounded-hw border px-2.5 py-1 text-[11px] outline-none"
          />
          <button
            type="button"
            style={{
              backgroundColor: colors.route,
              color: colors.onSignal,
              borderColor: colors.route,
            }}
            className="legend rounded-hw border px-3 py-1 text-[11px] font-medium"
          >
            DISPATCH
          </button>
        </div>

        <div className="flex items-center gap-3">
          <span className="legend text-[11px]" style={{ color: colors.inkDim }}>
            LEVER:
          </span>
          <button
            type="button"
            onClick={() => setToggleState(!toggleState)}
            style={{
              backgroundColor: toggleState ? colors.panelFace : colors.panelRecess,
              borderColor: toggleState ? colors.clear : colors.edgeStrong,
              color: toggleState ? colors.clear : colors.inkFaint,
            }}
            className="legend flex items-center gap-1.5 rounded-hw border px-2 py-0.5 text-[11px]"
          >
            <span
              style={{ backgroundColor: toggleState ? colors.clear : colors.lampOff }}
              className="size-[6px] rounded-hw-sm"
            />
            {toggleState ? "ON" : "OFF"}
          </button>
        </div>
      </div>
    </div>
  );
});

export function AppearanceView() {
  const {
    settings,
    update,
    setThemeMode,
    selectPreset,
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
    animationsEnabled,
    confettiOnComplete,
    setConfettiOnComplete,
    confettiSfx,
    setConfettiSfx,
    notificationLimit,
    setNotificationLimit,
    setHardwareAcceleration,
    setWebGpu,
    setEffect,
    setEffectIntensity,
    setGlassStyle,
    setNoisePlacement,
    toggleTelemetry,
    toggleColumn,
    resetColumns,
    resetDefaults,
    importThemeJson,
  } = useAppearance();

  const [presetCategory, setPresetCategory] = useState<string>("all");
  const [saveName, setSaveName] = useState("");
  const [showSaveInput, setShowSaveInput] = useState(false);
  const [jsonInput, setJsonInput] = useState("");
  const [showImportModal, setShowImportModal] = useState(false);
  const [showGlassModal, setShowGlassModal] = useState(false);
  const [importStatus, setImportStatus] = useState<string | null>(null);
  const [copiedNotification, setCopiedNotification] = useState(false);
  const [glassAnchor, setGlassAnchor] = useState({ x: 50, y: 68 });
  const [previewCardExpanded, setPreviewCardExpanded] = useState(false);

  const visibleColumns = useMemo(() => new Set(settings.columns), [settings.columns]);

  // Grouped presets
  const allPresets = useMemo(() => {
    const userAsThemePresets: ThemePreset[] = settings.userPresets.map((u) => ({
      id: u.id,
      name: u.name,
      description: u.description || "User-authored custom theme",
      category: "classic",
      colors: u.colors,
    }));
    return [...BUILTIN_PRESETS, ...userAsThemePresets];
  }, [settings.userPresets]);

  const filteredPresets = useMemo(() => {
    if (presetCategory === "all") return allPresets;
    if (presetCategory === "user") return allPresets.filter((p) => p.id.startsWith("user-"));
    return allPresets.filter((p) => p.category === presetCategory);
  }, [allPresets, presetCategory]);

  const isLiquidGlassThemeActive = Boolean(
    settings.effects.glass && settings.effects.glassThemeOverride !== false
  );

  const activeColors = useMemo(() => {
    if (isLiquidGlassThemeActive) return LIQUID_GLASS_COLORS;
    return settings.themeMode === "custom"
      ? settings.customColors
      : settings.themeMode === "light"
        ? DEFAULT_LIGHT_COLORS
        : settings.themeMode === "dark"
          ? DEFAULT_DARK_COLORS
          : allPresets.find((p) => p.id === settings.activePresetId)?.colors || DEFAULT_DARK_COLORS;
  }, [isLiquidGlassThemeActive, settings.themeMode, settings.customColors, settings.activePresetId, allPresets]);

  // Local studio draft colors — edits remain isolated in the Live Preview until explicitly saved
  const [draftColors, setDraftColors] = useState<CustomColors>(() => ({ ...activeColors }));
  const [isDraftDirty, setIsDraftDirty] = useState(false);
  const [saveSuccessNotice, setSaveSuccessNotice] = useState(false);

  // Automatically track activeColors when no unsaved studio draft exists
  useEffect(() => {
    if (!isDraftDirty) {
      setDraftColors({ ...activeColors });
    }
  }, [activeColors, isDraftDirty]);

  const handleDraftColorChange = useCallback((key: keyof CustomColors, value: string) => {
    setIsDraftDirty(true);
    setDraftColors((prev) => ({
      ...prev,
      [key]: value,
    }));
  }, []);

  const handleLoadPalette = useCallback((palette: CustomColors) => {
    setIsDraftDirty(true);
    setDraftColors({ ...palette });
  }, []);

  const handleDiscardDraft = useCallback(() => {
    setDraftColors({ ...activeColors });
    setIsDraftDirty(false);
  }, [activeColors]);

  const handleSaveAndApply = useCallback(() => {
    setAllCustomColors(draftColors);
    setIsDraftDirty(false);
    setSaveSuccessNotice(true);
    setTimeout(() => setSaveSuccessNotice(false), 3500);
  }, [draftColors, setAllCustomColors]);

  const hasUnsavedChanges = useMemo(() => {
    if (isDraftDirty) return true;
    if (settings.themeMode !== "custom") return false;
    return (Object.keys(draftColors) as (keyof CustomColors)[]).some(
      (k) => draftColors[k]?.toLowerCase() !== settings.customColors[k]?.toLowerCase(),
    );
  }, [isDraftDirty, settings.themeMode, settings.customColors, draftColors]);

  const handleCopyJson = () => {
    const { key: _, ...iconPalette } = resolvedIconPalette(settings, matchMedia("(prefers-color-scheme: dark)").matches);
    const exportData = {
      version: 1,
      appName: "Zoltraak",
      type: "theme-export",
      themeMode: "custom",
      activePresetId: "custom",
      colors: draftColors,
      density: settings.density,
      radius: settings.radius,
      fontMode: settings.fontMode,
      motion: settings.motion,
      effects: settings.effects,
      iconPalette,
      exportedAt: new Date().toISOString(),
    };
    void navigator.clipboard.writeText(JSON.stringify(exportData, null, 2));
    setCopiedNotification(true);
    setTimeout(() => setCopiedNotification(false), 2200);
  };

  const handleImportSubmit = () => {
    if (!jsonInput.trim()) {
      setImportStatus("Please enter or paste valid theme JSON.");
      return;
    }
    const res = importThemeJson(jsonInput);
    if (res.ok) {
      try {
        const parsed = JSON.parse(jsonInput) as { colors?: Partial<CustomColors> };
        if (parsed.colors) {
          setDraftColors((prev) => ({ ...prev, ...parsed.colors }));
          setIsDraftDirty(false);
        }
      } catch {
        // ignore JSON parse error for draft sync
      }
      setImportStatus("Theme successfully imported and applied!");
      setTimeout(() => {
        setShowImportModal(false);
        setJsonInput("");
        setImportStatus(null);
      }, 1000);
    } else {
      setImportStatus(`Error: ${res.error || "Could not parse theme JSON."}`);
    }
  };

  const handleSavePresetSubmit = () => {
    if (!saveName.trim()) return;
    saveUserPreset(saveName, undefined, draftColors);
    setSaveName("");
    setShowSaveInput(false);
  };

  return (
    <div className="flex flex-col gap-8 pb-12">
      <AppIconSettings />
      <ToggleRow
        title="Sync theme with browser extension"
        description="Use the app’s colors in the extension popup, download dialog, and floating controls. Turn off to restore the extension’s own appearance."
        checked={settings.syncExtensionTheme}
        onChange={(syncExtensionTheme) => update({ syncExtensionTheme })}
      />
      {/* 1. Panel Lighting Mode */}
      <Section
        title="Panel Lighting & Master Mode"
        description="Choose how Zoltraak renders its canvas — follow system day/night rhythm, enforce a high-contrast enamel tone, or select from curated theme presets."
        aside={
          isLiquidGlassThemeActive ? (
            <div className="flex items-center gap-2">
              <span className="rounded-hw border border-route/50 bg-route/15 px-2 py-0.5 text-[11px] font-mono text-route font-semibold">
                💧 Overridden by Liquid Glass
              </span>
              <button
                type="button"
                onClick={() => setEffect("glassThemeOverride", false)}
                className="focus-ring legend rounded-hw border border-edge px-2 py-0.5 text-[11px] text-ink-dim hover:border-route hover:text-route transition-colors"
              >
                Disable Override
              </button>
            </div>
          ) : undefined
        }
      >
        <SlideGroup
          role="radiogroup"
          aria-label="Panel lighting mode"
          className={`flex flex-wrap w-fit overflow-hidden rounded-hw border border-edge-strong transition-opacity ${
            isLiquidGlassThemeActive ? "opacity-50 pointer-events-none" : ""
          }`}
        >
          {[
            { value: "system" as ThemeMode, label: "Follow OS", Glyph: Monitor },
            { value: "light" as ThemeMode, label: "Day Enamel", Glyph: Sun },
            { value: "dark" as ThemeMode, label: "Night Shift", Glyph: Moon },
            { value: "preset" as ThemeMode, label: "Preset Gallery", Glyph: LayoutGrid },
            { value: "custom" as ThemeMode, label: "Custom Studio", Glyph: Palette },
          ].map(({ value, label, Glyph }) => {
            const isCurrent = settings.themeMode === value;
            return (
              <button
                key={value}
                type="button"
                role="radio"
                disabled={isLiquidGlassThemeActive}
                aria-checked={isCurrent}
                onClick={() => setThemeMode(value)}
                className={`focus-ring legend flex items-center gap-2 border-r border-edge px-3.5 py-2 text-[11px] transition-colors last:border-r-0 ${
                  isCurrent ? "text-on-signal" : "text-ink-dim hover:bg-face hover:text-ink"
                }`}
              >
                <Glyph className="size-3.5" />
                {label}
              </button>
            );
          })}
        </SlideGroup>
        {isLiquidGlassThemeActive && (
          <p className="text-[11.5px] text-ink-dim">
            Master lighting mode selection is locked while the dedicated Liquid Glass palette is active. Click <strong>Disable Override</strong> to restore manual lighting mode selection.
          </p>
        )}
      </Section>

      {/* 2. Theme Presets Gallery */}
      <Section
        title="Curated Theme Presets"
        description="Handcrafted telemetry and dispatch boards designed for high readability, terminal aesthetics, and visual immersion."
        aside={
          <div className="flex items-center gap-1.5 text-[11px]">
            {["all", "dark", "light", "retro", "contrast", "user"].map((cat) => (
              <button
                key={cat}
                type="button"
                disabled={isLiquidGlassThemeActive}
                onClick={() => setPresetCategory(cat)}
                className={`focus-ring legend rounded-hw border px-2 py-0.5 text-[11px] transition-colors ${
                  presetCategory === cat
                    ? "border-route bg-route text-on-signal"
                    : "border-edge text-ink-dim hover:border-edge-strong hover:text-ink"
                } ${isLiquidGlassThemeActive ? "opacity-50 cursor-not-allowed" : ""}`}
              >
                {cat.toUpperCase()}
              </button>
            ))}
          </div>
        }
      >
        {isLiquidGlassThemeActive && (
          <div className="flex flex-wrap items-center justify-between gap-2 rounded-hw border border-route/40 bg-route/10 p-3 text-[11px]">
            <div className="flex items-center gap-2">
              <span className="size-2 rounded-full bg-route animate-pulse" />
              <span className="text-ink">
                Preset selection is paused while the dedicated Liquid Glass palette is active.
              </span>
            </div>
            <button
              type="button"
              onClick={() => setEffect("glassThemeOverride", false)}
              className="focus-ring legend rounded-hw border border-route bg-route px-2.5 py-1 text-[11px] font-semibold text-on-signal transition-opacity hover:opacity-90"
            >
              Disable Theme Override
            </button>
          </div>
        )}

        <div className={`grid grid-cols-1 gap-3.5 sm:grid-cols-2 lg:grid-cols-3 transition-opacity ${
          isLiquidGlassThemeActive ? "pointer-events-none opacity-50" : ""
        }`}>
          {filteredPresets.map((preset) => {
            const isSelected = settings.themeMode === "preset" && settings.activePresetId === preset.id;
            const isUserCreated = preset.id.startsWith("user-");

            return (
              <div
                key={preset.id}
                onClick={() => !isLiquidGlassThemeActive && selectPreset(preset.id)}
                className={`group relative flex cursor-pointer flex-col justify-between rounded-hw-lg border p-3.5 transition-all ${
                  isSelected
                    ? "border-route bg-face shadow-md ring-1 ring-route"
                    : "border-edge bg-face/70 hover:border-edge-strong hover:bg-face"
                }`}
              >
                <div>
                  <div className="flex items-center justify-between gap-2">
                    <div className="flex items-center gap-2">
                      <span className="text-[13px] font-medium text-ink">{preset.name}</span>
                      {isSelected && (
                        <span className="legend flex items-center gap-1 rounded-hw-sm bg-route px-1.5 py-[1px] text-[11px] text-on-signal">
                          <Check className="size-2.5" /> ACTIVE
                        </span>
                      )}
                    </div>

                    {isUserCreated && (
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation();
                          deleteUserPreset(preset.id);
                        }}
                        className="focus-ring rounded-hw p-1 text-ink-faint transition-colors hover:bg-recess hover:text-danger"
                        title="Delete custom preset"
                      >
                        <Trash className="size-3" />
                      </button>
                    )}
                  </div>

                  <p className="mt-1 text-[11px] leading-relaxed text-ink-dim">{preset.description}</p>
                </div>

                {/* 4-Slot Mini Swatch Palette */}
                <div className="mt-3.5 flex items-center gap-2 border-t border-edge/60 pt-2.5">
                  <div className="flex flex-1 items-center overflow-hidden rounded-hw border border-edge">
                    <div
                      style={{ backgroundColor: preset.colors.panel }}
                      className="h-3 flex-1"
                      title={`Panel: ${preset.colors.panel}`}
                    />
                    <div
                      style={{ backgroundColor: preset.colors.panelFace }}
                      className="h-3 flex-1"
                      title={`Face: ${preset.colors.panelFace}`}
                    />
                    <div
                      style={{ backgroundColor: preset.colors.route }}
                      className="h-3 flex-1"
                      title={`Route Accent: ${preset.colors.route}`}
                    />
                    <div
                      style={{ backgroundColor: preset.colors.ink }}
                      className="h-3 flex-1"
                      title={`Ink: ${preset.colors.ink}`}
                    />
                  </div>
                  <span className="describer text-[11px] text-ink-faint">{preset.category}</span>
                </div>
              </div>
            );
          })}
        </div>
      </Section>

      {/* 3. Live Custom Theme Studio */}
      <Section
        title="Custom Theme Studio"
        description="Fine-tune individual surface enamel, boundary hairlines, signal lamps, and lettering. Any edits update the live preview below in real-time. Click 'Save & Apply Theme' to activate across the entire application."
        aside={
          <div className="flex flex-wrap items-center gap-2">
            {hasUnsavedChanges && (
              <Button tone="ghost" onClick={handleDiscardDraft}>
                Discard Draft
              </Button>
            )}
            <Button
              tone="primary"
              disabled={isLiquidGlassThemeActive}
              onClick={handleSaveAndApply}
              className={hasUnsavedChanges && !isLiquidGlassThemeActive ? "ring-1 ring-route shadow-sm" : ""}
            >
              <span className="flex items-center gap-1.5">
                <Check className="size-3" />
                Save & Apply Theme
              </span>
            </Button>
            <Button tone="ghost" onClick={() => setShowSaveInput(true)}>
              Save as Preset
            </Button>
            <Button tone="ghost" onClick={handleCopyJson}>
              <span className="flex items-center gap-1.5">
                <Copy className="size-3" />
                {copiedNotification ? "Copied!" : "Export JSON"}
              </span>
            </Button>
            <Button tone="ghost" onClick={() => setShowImportModal(true)}>
              <span className="flex items-center gap-1.5">
                <FileUp className="size-3" />
                Import
              </span>
            </Button>
          </div>
        }
      >
        {/* Liquid Glass Theme Override banner */}
        {isLiquidGlassThemeActive && (
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-hw border border-route/40 bg-route/10 px-3.5 py-2.5 text-[12px] text-route">
            <div className="flex items-center gap-2">
              <span className="size-2 rounded-full bg-route animate-pulse shrink-0" />
              <span className="text-ink">
                <strong>Liquid Glass Dedicated Theme Active:</strong> Custom color editing is paused while the dedicated Liquid Glass optical palette is active.
              </span>
            </div>
            <button
              type="button"
              onClick={() => setEffect("glassThemeOverride", false)}
              className="focus-ring rounded-hw bg-route px-2.5 py-1 text-[11px] font-semibold text-on-signal hover:opacity-90 active:scale-95 transition-transform"
            >
              Disable Theme Override
            </button>
          </div>
        )}

        {/* Save preset input drawer */}
        {showSaveInput && (
          <div className="flex flex-col gap-2 rounded-hw border border-route/40 bg-face p-3.5">
            <Legend className="text-ink">Save Draft Colors as Preset</Legend>
            <div className="flex items-center gap-2">
              <TextInput
                value={saveName}
                onChange={setSaveName}
                placeholder="e.g. Cobalt Midnight Station"
                ariaLabel="New preset name"
                className="max-w-md"
              />
              <Button onClick={handleSavePresetSubmit}>Save Preset</Button>
              <Button tone="ghost" onClick={() => setShowSaveInput(false)}>
                Cancel
              </Button>
            </div>
          </div>
        )}

        {/* Unsaved draft alert or Save confirmation notice */}
        {saveSuccessNotice && (
          <div className="flex items-center justify-between gap-3 rounded-hw border border-clear/50 bg-clear/10 px-3.5 py-2.5 text-[12px] text-clear">
            <div className="flex items-center gap-2 font-medium">
              <Check className="size-4" />
              <span>Theme saved and applied to Zoltraak! The entire application now uses your custom palette.</span>
            </div>
          </div>
        )}

        {hasUnsavedChanges && !saveSuccessNotice && (
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-hw border border-caution/40 bg-caution/10 px-3.5 py-2 text-[12px] text-caution">
            <div className="flex items-center gap-2">
              <span className="size-2 rounded-full bg-caution animate-ping" />
              <span>
                You are editing a custom theme draft. Changes are only rendered in the Live Preview below until you save.
              </span>
            </div>
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={handleDiscardDraft}
                className="focus-ring text-[11px] underline text-ink-dim hover:text-ink"
              >
                Discard Draft
              </button>
              <button
                type="button"
                onClick={handleSaveAndApply}
                className="focus-ring rounded-hw bg-route px-3 py-1 text-[11px] font-semibold text-on-signal shadow-sm transition-opacity hover:opacity-90 active:scale-95"
              >
                Save & Apply Theme
              </button>
            </div>
          </div>
        )}

        {/* Live Mini Preview Console */}
        <div className="flex flex-col gap-2">
          <div className="flex items-center justify-between">
            <Legend className="text-ink">Interactive Telemetry Simulator</Legend>
            <span className="text-[11px] text-ink-faint">
              {hasUnsavedChanges ? "Showing draft preview (isolated)" : "Synchronized with active app colors"}
            </span>
          </div>
          <LiveDispatchPreview
            colors={draftColors}
            density={settings.density}
            radius={settings.radius}
            hasUnsavedChanges={hasUnsavedChanges}
            onSave={handleSaveAndApply}
          />
        </div>

        {/* Color Palette Grid */}
        <div className={`grid grid-cols-1 gap-6 md:grid-cols-2 lg:grid-cols-4 transition-opacity ${
          isLiquidGlassThemeActive ? "pointer-events-none opacity-50" : ""
        }`}>
          {["Surfaces", "Boundaries", "Lettering", "Signals & Lamps"].map((groupName) => (
            <div key={groupName} className="flex flex-col gap-3 rounded-hw-lg border border-edge bg-face/50 p-3.5">
              <div className="border-b border-edge pb-1.5">
                <Legend className="text-ink font-semibold">{groupName}</Legend>
              </div>

              <div className="flex flex-col gap-3">
                {COLOR_SLOT_LABELS.filter((s) => s.group === groupName).map(({ key, label, hint }) => (
                  <ColorSlotItem
                    key={key}
                    slotKey={key}
                    label={label}
                    hint={hint}
                    value={draftColors[key] || "#000000"}
                    onChange={handleDraftColorChange}
                  />
                ))}
              </div>
            </div>
          ))}
        </div>

        {/* Quick color utility actions & save */}
        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-edge pt-4">
          <div className="flex flex-wrap items-center gap-2">
            <Button
              tone="primary"
              onClick={handleSaveAndApply}
              className={hasUnsavedChanges ? "ring-1 ring-route shadow-sm" : ""}
            >
              <span className="flex items-center gap-1.5">
                <Check className="size-3" />
                Save & Apply Theme to App
              </span>
            </Button>
            {hasUnsavedChanges && (
              <Button tone="ghost" onClick={handleDiscardDraft} className="text-[11px]">
                Discard Changes
              </Button>
            )}
            <Button
              tone="ghost"
              onClick={() => handleLoadPalette(DEFAULT_LIGHT_COLORS)}
              className="text-[11px]"
            >
              Load Day Palette
            </Button>
            <Button
              tone="ghost"
              onClick={() => handleLoadPalette(DEFAULT_DARK_COLORS)}
              className="text-[11px]"
            >
              Load Night Palette
            </Button>
          </div>
          <div className="flex items-center gap-2">
            {saveSuccessNotice && (
              <span className="legend flex items-center gap-1 text-[11px] text-clear font-medium animate-pulse">
                <Check className="size-3.5" />
                Theme saved and applied to application!
              </span>
            )}
            <span className="text-[11px] text-ink-faint">
              Edits update the live preview above without affecting the active app until saved.
            </span>
          </div>
        </div>
      </Section>

      {/* 4. Display Geometry & Density */}
      <Section
        title="Interface Geometry & Sizing"
        description="Scale the entire interface to fit high-density or widescreen displays, and adjust row tightness to balance telemetry density against breathing room."
      >
        <div className="grid grid-cols-1 gap-6 sm:grid-cols-3">
          {/* UI Scale */}
          <Field
            label="Interface Zoom & Scale"
            hint={`Current scale: ${settings.uiScale}%. Fine-tune for high-DPI displays.`}
          >
            <div className="flex flex-col gap-2">
              <SlideGroup className="flex overflow-hidden rounded-hw border border-edge-strong">
                {[85, 90, 100, 110, 120].map((scale) => (
                  <button
                    key={scale}
                    type="button"
                    data-active={settings.uiScale === scale}
                    onClick={() => setUiScale(scale)}
                    className={`focus-ring legend flex-1 border-r border-edge py-1.5 text-[11px] transition-colors last:border-r-0 ${
                      settings.uiScale === scale
                        ? "text-on-signal"
                        : "text-ink-dim hover:bg-face hover:text-ink"
                    }`}
                  >
                    {scale}%
                  </button>
                ))}
              </SlideGroup>
              <input
                type="range"
                min="80"
                max="125"
                step="5"
                value={settings.uiScale}
                onChange={(e) => setUiScale(Number(e.target.value))}
                className="accent-route h-1.5 cursor-pointer rounded-hw bg-recess"
              />
            </div>
          </Field>

          {/* Layout Density */}
          <Field
            label="Row & Table Density"
            hint="Spacing for rows, tables, settings, dialogs and the guide, and the browser extension while theme sync is on. Compact fits more on screen; comfortable gives wider touch targets."
          >
            <SlideGroup className="flex overflow-hidden rounded-hw border border-edge-strong">
              {[
                { value: "compact" as Density, label: "Compact" },
                { value: "default" as Density, label: "Standard" },
                { value: "comfortable" as Density, label: "Relaxed" },
              ].map(({ value, label }) => (
                <button
                  key={value}
                  type="button"
                  data-active={settings.density === value}
                  onClick={() => setDensity(value)}
                  className={`focus-ring legend flex-1 border-r border-edge py-1.5 text-[11px] transition-colors last:border-r-0 ${
                    settings.density === value
                      ? "text-on-signal"
                      : "text-ink-dim hover:bg-face hover:text-ink"
                  }`}
                >
                  {label}
                </button>
              ))}
            </SlideGroup>
          </Field>

          {/* Corner Geometry */}
          <Field
            label="Hardware Corner Bevel"
            hint="Corner shape for every container and control, the window itself on Windows 11, the guide, and the browser extension while theme sync is on. Sharp bevels mimic industrial consoles; rounded corners are softer."
          >
            <SlideGroup className="flex overflow-hidden rounded-hw border border-edge-strong">
              {[
                { value: "sharp" as CornerRadius, label: "Sharp 0px" },
                { value: "bevel" as CornerRadius, label: "Bevel 2px" },
                { value: "modern" as CornerRadius, label: "Soft 6px" },
                { value: "rounded" as CornerRadius, label: "Round 10px" },
              ].map(({ value, label }) => (
                <button
                  key={value}
                  type="button"
                  data-active={settings.radius === value}
                  onClick={() => setRadius(value)}
                  className={`focus-ring legend flex-1 border-r border-edge py-1.5 text-[11px] transition-colors last:border-r-0 ${
                    settings.radius === value
                      ? "text-on-signal"
                      : "text-ink-dim hover:bg-face hover:text-ink"
                  }`}
                >
                  {label}
                </button>
              ))}
            </SlideGroup>
          </Field>
        </div>
      </Section>

      {/* 5. Typography & Atmosphere */}
      <Section
        title="Typography & Atmospheric Overlays"
        description="Tailor typeface hierarchy and enable vintage CRT raster line overlays or schematic blueprint grids."
      >
        {/* Typography Mode */}
        <Field
          label="Typeface Character"
          hint="Each option is shown in the faces it applies. Upload your own font to use it across the interface; the guide and the browser extension keep their own typefaces."
        >
          <TypefacePicker mode={settings.fontMode} customFont={settings.customFont} onMode={setFontMode} onCustomFont={setCustomFont} />
        </Field>

        <div className="mt-4 divide-y divide-edge border-t border-edge">
          <AtmosphericEffectRow
            title="CRT Phosphor Scanlines"
            badge="Raster"
            description="Adds an authentic cathode-ray tube horizontal raster line overlay with RGB chromatic dispersion."
            checked={settings.effects.scanlines}
            onToggle={(next) => setEffect("scanlines", next)}
            intensity={settings.effects.scanlinesIntensity ?? 70}
            onIntensityChange={(val) => setEffectIntensity("scanlinesIntensity", val)}
            intensityLabel="Raster Opacity"
          />
          <AtmosphericEffectRow
            title="Schematic Millimeter Grid"
            badge="Geometry"
            description="Renders subtle etched draftsman blueprint gridlines behind the panel enamel canvas."
            checked={settings.effects.grid}
            onToggle={(next) => setEffect("grid", next)}
            intensity={settings.effects.gridIntensity ?? 50}
            onIntensityChange={(val) => setEffectIntensity("gridIntensity", val)}
            intensityLabel="Grid Contrast"
          />
          <AtmosphericEffectRow
            title="Backdrop Glassmorphism (Liquid Glass)"
            badge="Optics"
            tag={
              <span className="inline-flex items-center gap-1 rounded-hw border border-caution/40 bg-caution/15 px-1.5 py-[1px] text-[11px] font-mono font-bold uppercase tracking-wider text-caution">
                <span className="size-1 rounded-full bg-caution animate-pulse" />
                Beta
              </span>
            }
            description="Transforms the navbar into an elevated floating pill design, applying Liquid Glass specular caustics, meniscus curvature, chromatic dispersion, and frosted diffusion to dialogs, settings, and elevated chrome."
            checked={settings.effects.glass}
            onToggle={(next) => setEffect("glass", next)}
            intensity={settings.effects.glassIntensity ?? 60}
            onIntensityChange={(val) => setEffectIntensity("glassIntensity", val)}
            intensityLabel="Refraction & Blur"
            action={
              <div className="flex flex-wrap items-center gap-2">
                <SlideGroup className="flex overflow-hidden rounded-hw border border-edge-strong bg-face/70">
                  <button
                    type="button"
                    onClick={() => setGlassStyle("liquid")}
                    className={`focus-ring px-2.5 py-1 text-[11px] font-mono transition-colors ${
                      (settings.effects.glassStyle ?? "liquid") === "liquid"
                        ? "bg-route text-on-signal font-semibold"
                        : "text-ink-dim hover:text-ink"
                    }`}
                  >
                    💧 Liquid Glass
                  </button>
                  <button
                    type="button"
                    data-active={settings.effects.glassStyle === "frosted"}
                    onClick={() => setGlassStyle("frosted")}
                    className={`focus-ring border-l border-edge px-2.5 py-1 text-[11px] font-mono transition-colors ${
                      settings.effects.glassStyle === "frosted"
                        ? "text-on-signal font-semibold"
                        : "text-ink-dim hover:text-ink"
                    }`}
                  >
                    ❄ Matte Frost
                  </button>
                </SlideGroup>
                <button
                  type="button"
                  onClick={() => setShowGlassModal(true)}
                  className="focus-ring legend flex items-center gap-1 rounded-hw border border-route/50 bg-route/15 px-2.5 py-1 text-[11px] font-medium text-route transition-colors hover:bg-route hover:text-on-signal"
                >
                  <span>Test Liquid Glass</span>
                  <ArrowUpRight className="size-2.5" />
                </button>
              </div>
            }
          >
            <div className="mt-3.5 space-y-3 rounded-hw-lg border border-edge/70 bg-face/30 p-3">
              {/* Theme Override Toggle Bar */}
              <div className="flex flex-wrap items-center justify-between gap-2 border-b border-edge/60 pb-2.5">
                <div>
                  <div className="flex items-center gap-1.5 text-[12px] font-semibold text-ink">
                    <span>🎨 Dedicated Glass Palette Override</span>
                    <span
                      className={`rounded-hw px-1.5 py-[1px] text-[11px] font-mono uppercase tracking-wider ${
                        settings.effects.glassThemeOverride !== false
                          ? "border border-route/50 bg-route/20 text-route font-semibold"
                          : "border border-edge bg-recess text-ink-faint"
                      }`}
                    >
                      {settings.effects.glassThemeOverride !== false ? "Active" : "Disabled"}
                    </span>
                  </div>
                  <p className="mt-0.5 text-[11px] text-ink-dim">
                    Locks color palette to deep obsidian, quartz, and photonic cyan tuned specifically for liquid glass optics.
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => setEffect("glassThemeOverride", settings.effects.glassThemeOverride === false)}
                  className={`focus-ring rounded-hw border px-2.5 py-1 text-[11px] font-medium transition-all ${
                    settings.effects.glassThemeOverride !== false
                      ? "border-route bg-route text-on-signal hover:opacity-90 active:scale-95"
                      : "border-edge-strong bg-face text-ink hover:border-route hover:text-route active:scale-95"
                  }`}
                >
                  {settings.effects.glassThemeOverride !== false ? "Disable Theme Override" : "Enable Theme Override"}
                </button>
              </div>

              {/* Surface Targets Toggle Buttons */}
              <div>
                <div className="mb-2 flex items-center justify-between">
                  <span className="text-[11px] font-mono uppercase tracking-wider text-ink-faint">
                    Surface Glass Toggles
                  </span>
                  <span className="text-[11px] text-ink-dim">
                    Toggle glassmorphism styling for specific UI surfaces
                  </span>
                </div>

                <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                  {/* Target 1: Floating Pill Navbar */}
                  <div className="flex items-center justify-between gap-2 rounded-hw border border-edge bg-face/60 p-2.5">
                    <div className="min-w-0">
                      <span className="text-[11px] font-semibold text-ink">Floating Pill Navbar</span>
                      <p className="text-[11px] text-ink-dim leading-tight">
                        Floating header bar with caustics, specular edges & pill geometry
                      </p>
                    </div>
                    <button
                      type="button"
                      onClick={() => setEffect("glassNavbar", settings.effects.glassNavbar === false)}
                      className={`focus-ring shrink-0 rounded-hw border px-2 py-0.5 text-[11px] font-mono uppercase transition-colors ${
                        settings.effects.glassNavbar !== false
                          ? "border-route bg-route text-on-signal font-semibold"
                          : "border-edge-strong bg-recess text-ink-dim hover:text-ink"
                      }`}
                    >
                      {settings.effects.glassNavbar !== false ? "ON" : "OFF"}
                    </button>
                  </div>

                  {/* Target 2: Modals */}
                  <div className="flex items-center justify-between gap-2 rounded-hw border border-edge bg-face/60 p-2.5">
                    <div className="min-w-0">
                      <span className="text-[11px] font-semibold text-ink">In-App Modals</span>
                      <p className="text-[11px] text-ink-dim leading-tight">
                        Dialog windows, confirmation plates & frosted backdrop blur
                      </p>
                    </div>
                    <button
                      type="button"
                      onClick={() => setEffect("glassModals", settings.effects.glassModals === false)}
                      className={`focus-ring shrink-0 rounded-hw border px-2 py-0.5 text-[11px] font-mono uppercase transition-colors ${
                        settings.effects.glassModals !== false
                          ? "border-route bg-route text-on-signal font-semibold"
                          : "border-edge-strong bg-recess text-ink-dim hover:text-ink"
                      }`}
                    >
                      {settings.effects.glassModals !== false ? "ON" : "OFF"}
                    </button>
                  </div>

                  {/* Target 3: Settings Shell */}
                  <div className="flex items-center justify-between gap-2 rounded-hw border border-edge bg-face/60 p-2.5">
                    <div className="min-w-0">
                      <span className="text-[11px] font-semibold text-ink">Settings Shell & Rail</span>
                      <p className="text-[11px] text-ink-dim leading-tight">
                        Tabbed sidebar navigation rail & settings outline cards
                      </p>
                    </div>
                    <button
                      type="button"
                      onClick={() => setEffect("glassSettings", settings.effects.glassSettings === false)}
                      className={`focus-ring shrink-0 rounded-hw border px-2 py-0.5 text-[11px] font-mono uppercase transition-colors ${
                        settings.effects.glassSettings !== false
                          ? "border-route bg-route text-on-signal font-semibold"
                          : "border-edge-strong bg-recess text-ink-dim hover:text-ink"
                      }`}
                    >
                      {settings.effects.glassSettings !== false ? "ON" : "OFF"}
                    </button>
                  </div>

                  {/* Target 4: Overlays */}
                  <div className="flex items-center justify-between gap-2 rounded-hw border border-edge bg-face/60 p-2.5">
                    <div className="min-w-0">
                      <span className="text-[11px] font-semibold text-ink">Menus & Toasts</span>
                      <p className="text-[11px] text-ink-dim leading-tight">
                        Floating context menus, action popovers & status toast alerts
                      </p>
                    </div>
                    <button
                      type="button"
                      onClick={() => setEffect("glassOverlays", settings.effects.glassOverlays === false)}
                      className={`focus-ring shrink-0 rounded-hw border px-2 py-0.5 text-[11px] font-mono uppercase transition-colors ${
                        settings.effects.glassOverlays !== false
                          ? "border-route bg-route text-on-signal font-semibold"
                          : "border-edge-strong bg-recess text-ink-dim hover:text-ink"
                      }`}
                    >
                      {settings.effects.glassOverlays !== false ? "ON" : "OFF"}
                    </button>
                  </div>
                </div>
              </div>
            </div>
          </AtmosphericEffectRow>
          <AtmosphericEffectRow
            title="Cathode Screen Vignette"
            badge="Curvature"
            description="Deepens perimeter shadow falloff and curved CRT bezel darkening toward display edges."
            checked={settings.effects.vignette}
            onToggle={(next) => setEffect("vignette", next)}
            intensity={settings.effects.vignetteIntensity ?? 50}
            onIntensityChange={(val) => setEffectIntensity("vignetteIntensity", val)}
            intensityLabel="Vignette Falloff"
          />
          <AtmosphericEffectRow
            title="Phosphor Bloom & Halation"
            badge="Radiance"
            description="Simulates luminous light bleed and electron flare radiating around active signals, lamps, and telemetry readouts."
            checked={settings.effects.bloom}
            onToggle={(next) => setEffect("bloom", next)}
            intensity={settings.effects.bloomIntensity ?? 50}
            onIntensityChange={(val) => setEffectIntensity("bloomIntensity", val)}
            intensityLabel="Bloom Halation"
          />
          <AtmosphericEffectRow
            title="Analogue Console Noise"
            badge="Thermal"
            description="Superimposes subtle high-frequency analog thermal noise and physical plate grain across surfaces."
            checked={settings.effects.noise}
            onToggle={(next) => setEffect("noise", next)}
            intensity={settings.effects.noiseIntensity ?? 35}
            onIntensityChange={(val) => setEffectIntensity("noiseIntensity", val)}
            intensityLabel="Grain Density"
            controls={
              <div className={`mt-3 flex items-center gap-3 transition-opacity duration-150 ${settings.effects.noise ? "opacity-100" : "opacity-40"}`}>
                <span className="min-w-[110px] text-[11px] font-mono uppercase tracking-wider text-ink-faint">Grain size</span>
                <input type="range" min={0.5} max={4} step={0.25}
                  value={settings.effects.noiseGrainSize}
                  disabled={!settings.effects.noise}
                  onChange={event => update({ effects: { ...settings.effects, noiseGrainSize: Number(event.target.value) } })}
                  aria-label="Analogue Console Noise Grain size"
                  className="accent-route h-1.5 min-w-0 flex-1 cursor-pointer rounded-hw bg-recess disabled:cursor-not-allowed" />
                <span className="min-w-[42px] text-right font-mono text-[11px] font-semibold text-ink">{settings.effects.noiseGrainSize}×</span>
              </div>
            }
            action={
              <SlideGroup className="flex overflow-hidden rounded-hw border border-edge-strong bg-face/70">
                <button
                  type="button"
                  data-active={settings.effects.noisePlacement === "underlay"}
                  onClick={() => setNoisePlacement("underlay")}
                  className={`focus-ring px-2.5 py-1 text-[11px] font-mono transition-colors ${
                    settings.effects.noisePlacement === "underlay"
                      ? "text-on-signal font-semibold"
                      : "text-ink-dim hover:text-ink"
                  }`}
                  title="Render noise as an underlay background behind all interface chrome, plates, cards, and text"
                >
                  ⊟ Underlay
                </button>
                <button
                  type="button"
                  onClick={() => setNoisePlacement("overlay")}
                  className={`focus-ring border-l border-edge px-2.5 py-1 text-[11px] font-mono transition-colors ${
                    (settings.effects.noisePlacement ?? "overlay") === "overlay"
                      ? "bg-route text-on-signal font-semibold"
                      : "text-ink-dim hover:text-ink"
                  }`}
                  title="Render noise as an overlay across the screen and display"
                >
                  ⊞ Overlay
                </button>
              </SlideGroup>
            }
          />
          <AtmosphericEffectRow
            title="Ambient Filament Warmth"
            badge="Filament"
            description="Casts a warm incandescent vacuum tube filament glow wash across the upper chassis backplane."
            checked={settings.effects.warmth}
            onToggle={(next) => setEffect("warmth", next)}
            intensity={settings.effects.warmthIntensity ?? 40}
            onIntensityChange={(val) => setEffectIntensity("warmthIntensity", val)}
            intensityLabel="Filament Glow"
          />
        </div>
      </Section>

      {/* 6. Motion & Interface Animations */}
      <Section
        title="Motion & Interface Animations"
        description="Control transitions, mechanical relay physics, block throw sweeps on THE LINE, and card resize animations. Disable animations for zero motion and lowest latency."
      >
        <div className="flex flex-col gap-4">
          <ToggleRow
            title="Enable Interface Animations & Transitions"
            description="Activate card resize morphs, accordion cutaways, floating toasts, origin-aware menus, and mechanical lever settling. Turn off for zero-latency, instantaneous switching across the whole interface."
            checked={animationsEnabled}
            onChange={setAnimationsEnabled}
          />

          <Field
            label="Transition Dynamics Profile"
            hint={
              settings.motion === "full"
                ? "Full dynamics runs 250–400ms physical curves with soft depth blur and expressive ease."
                : settings.motion === "subtle"
                  ? "Subtle runs tight 120–160ms micro-interactions with reduced displacement and zero blur."
                  : "Instantaneous switches state with zero motion latency (0ms duration)."
            }
          >
            <SlideGroup className="flex overflow-hidden rounded-hw border border-edge-strong">
              {[
                { value: "full" as MotionMode, label: "Full (Expressive)" },
                { value: "subtle" as MotionMode, label: "Subtle (Snappy)" },
                { value: "reduced" as MotionMode, label: "Instant (Off)" },
              ].map(({ value, label }) => (
                <button
                  key={value}
                  type="button"
                  data-active={settings.motion === value}
                  onClick={() => setMotion(value)}
                  className={`focus-ring legend flex-1 border-r border-edge py-1.5 text-[11px] transition-colors last:border-r-0 ${
                    settings.motion === value
                      ? "text-on-signal"
                      : "text-ink-dim hover:bg-face hover:text-ink"
                  }`}
                >
                  {label}
                </button>
              ))}
            </SlideGroup>
          </Field>

            <Field label="Download double-click interval" hint="Maximum time between two clicks to open a file. Lower values require faster clicks. Double-clicking the description column does not open files.">
              <div className="flex items-center gap-3">
                <input type="range" aria-label="Download double-click interval"
                  min={100} max={600} step={50}
                  value={settings.downloadDoubleClickInterval}
                  onChange={event => update({ downloadDoubleClickInterval: Number(event.target.value) })}
                  className="min-w-0 flex-1 accent-route" />
                <span className="describer w-14 text-right text-[11px] text-ink-dim">{settings.downloadDoubleClickInterval} ms</span>
              </div>
            </Field>
            <ToggleRow
              title="File opening feedback"
              description="Briefly tint a download entry when opening it with double-click, a bound mouse button, or a keyboard shortcut."
              checked={settings.fileOpenFeedback}
              onChange={fileOpenFeedback => update({ fileOpenFeedback })}
            />
            <Field label="File opening feedback duration" hint="Requires interface animations. Reduced-motion preferences shorten the flash.">
              <div className="flex items-center gap-3">
                <input
                  type="range"
                  aria-label="File opening feedback duration"
                  min={100} max={600} step={50}
                  value={settings.fileOpenFeedbackDuration}
                  disabled={!settings.fileOpenFeedback || !animationsEnabled}
                  onChange={event => update({ fileOpenFeedbackDuration: Number(event.target.value) })}
                  className="min-w-0 flex-1 accent-route disabled:opacity-45"
                />
                <span className="describer w-14 text-right text-[11px] text-ink-dim">{settings.fileOpenFeedbackDuration} ms</span>
                <button type="button" disabled={!settings.fileOpenFeedback || !animationsEnabled}
                  onClick={event => flashFileOpen(event.currentTarget, settings)}
                  className="focus-ring rounded-hw border border-edge-strong px-3 py-1.5 text-[11px] text-ink disabled:opacity-45">
                  Preview
                </button>
              </div>
            </Field>

            {/* Live Card Resize Preview Sandbox */}
          <div className="mt-1 rounded-hw-lg border border-edge bg-recess/50 p-3.5">
            <div className="flex items-center justify-between">
              <div>
                <p className="text-[12px] font-medium text-ink">Live Transition Preview</p>
                <p className="text-[11px] text-ink-dim">
                  Preview card resizing with the current motion profile:{" "}
                  <span className="font-mono text-route font-medium uppercase">{settings.motion}</span>
                </p>
              </div>
              <button
                type="button"
                onClick={() => setPreviewCardExpanded((prev) => !prev)}
                className="focus-ring legend rounded-hw border border-edge bg-face px-3 py-1 text-[11px] text-ink transition-colors hover:border-edge-strong hover:bg-panel"
              >
                {previewCardExpanded ? "Compact View" : "Expand Card"}
              </button>
            </div>

            <div
              className={`t-resize plate mt-3 overflow-hidden rounded-hw-lg border border-edge bg-face transition-[height,background-color] ${
                previewCardExpanded ? "h-28 bg-face p-3.5 shadow-sm" : "h-12 bg-recess/70 px-3.5 py-2"
              }`}
            >
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <span className="size-2 rounded-full bg-route" />
                  <span className="text-[12px] font-mono text-ink">
                    {previewCardExpanded ? "BUFFER_RESIZE_TARGET [112px]" : "BUFFER_COMPACT [48px]"}
                  </span>
                </div>
                <span className="text-[11px] font-mono text-ink-faint">
                  {settings.motion === "full"
                    ? "300ms cubic-bezier(0.22, 1, 0.36, 1)"
                    : settings.motion === "subtle"
                      ? "150ms cubic-bezier(0.22, 1, 0.36, 1)"
                      : "0ms (Instantaneous)"}
                </span>
              </div>
              {previewCardExpanded && (
                <div className="mt-2.5 space-y-1 text-[11px] text-ink-dim">
                  <p>✓ Card height transitions smoothly</p>
                  <p>✓ Hardware acceleration & responsive physics active</p>
                </div>
              )}
            </div>
          </div>

          {/* Download Finished Confetti Celebration Toggle */}
          <div className="flex items-center justify-between border-t border-edge pt-3.5">
            <div className="flex-1 pr-4">
              <div className="flex items-center gap-2">
                <p className="text-[12px] font-medium text-ink">Download Finished Confetti Burst</p>
                <span className="rounded-hw border border-edge bg-recess px-1.5 py-0.2 text-[11px] font-mono text-ink-dim uppercase">
                  Celebration
                </span>
              </div>
              <p className="mt-0.5 text-[11px] text-ink-dim">
                Celebrate completed file transfers with an expressive full-width confetti cascade falling from top to bottom across your screen.
              </p>
            </div>
            <div className="flex items-center gap-3">
              <button
                type="button"
                onClick={() => triggerConfetti(true)}
                className="focus-ring legend flex items-center gap-1.5 rounded-hw border border-edge bg-face px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:border-edge-strong hover:bg-panel hover:text-ink"
                title="Test confetti celebration burst"
              >
                <span>🎉</span>
                <span>Test Burst</span>
              </button>
              <Switch
                label="Download Finished Confetti Burst"
                checked={confettiOnComplete}
                onChange={setConfettiOnComplete}
              />
            </div>
          </div>

          {/* Confetti Audio SFX Toggle */}
          <div className="flex items-center justify-between border-t border-edge/60 pt-3">
            <div className="flex-1 pr-4">
              <div className="flex items-center gap-2">
                <p className="text-[12px] font-medium text-ink">Confetti Celebration Sound Effect</p>
                <span className="rounded-hw border border-edge bg-recess px-1.5 py-0.2 text-[11px] font-mono text-ink-dim uppercase">
                  SFX
                </span>
              </div>
              <p className="mt-0.5 text-[11px] text-ink-dim">
                Play a cheerful party-popper pop and sparkling chime cascade alongside the confetti burst.
              </p>
            </div>
            <div className="flex items-center gap-3">
              <Switch
                label="Confetti Celebration Sound Effect"
                checked={confettiSfx}
                onChange={setConfettiSfx}
                disabled={!confettiOnComplete}
              />
            </div>
          </div>

          {/* Notification History Limit */}
          <div className="flex items-center justify-between border-t border-edge/60 pt-3">
            <div className="flex-1 pr-4">
              <div className="flex items-center gap-2">
                <p className="text-[12px] font-medium text-ink">Notification History Limit</p>
                <span className="rounded-hw border border-edge bg-recess px-1.5 py-0.2 text-[11px] font-mono text-ink-dim uppercase">
                  Log
                </span>
              </div>
              <p className="mt-0.5 text-[11px] text-ink-dim">
                Maximum number of completed transfers and status alerts retained in the desk notifications inbox (defaults to 25).
              </p>
            </div>
            <div className="flex items-center gap-2">
              <NumberInput
                min={5}
                max={100}
                step={5}
                value={String(notificationLimit || 25)}
                onChange={value => {
                  const val = parseInt(value, 10);
                  if (!isNaN(val)) setNotificationLimit(val);
                }}
                className="w-20" size="sm"
                ariaLabel="Notification History Limit"
              />
              <span className="text-[11px] text-ink-faint">entries</span>
            </div>
          </div>
        </div>
      </Section>

      {/* 7. Hardware Acceleration & WebGPU Engine */}
      <Section
        id="section-hardware-acceleration-webgpu-subsystem"
        title="Visual Effects & WebGPU Waveform"
        description="Control interface effects and the waveform renderer. These controls leave the desktop webview's graphics acceleration unchanged."
      >
        <div className="flex flex-col gap-5">
          <div className="divide-y divide-edge border-b border-edge">
            <ToggleRow
              title="Enable Accelerated Visual Effects"
              description="Enable glass effects and waveform rendering. Turning this off reduces visual work without changing the webview's graphics backend."
              checked={settings.hardwareAcceleration}
              onChange={setHardwareAcceleration}
            />
            <ToggleRow
              title="WebGPU Shader Graphics (WGSL)"
              description="Utilize modern native WebGPU (Direct3D 12 / Vulkan / Metal) for high-performance oscillograph telemetry and phosphor shaders. Automatically falls back to Canvas 2D when disabled."
              checked={settings.webGpu}
              onChange={setWebGpu}
            />
          </div>

          <GpuVisualizer
            hardwareAcceleration={settings.hardwareAcceleration}
            webGpu={settings.webGpu}
            primaryColor={activeColors.route}
            backgroundColor={activeColors.panelRecess}
          />
        </div>
      </Section>

      {/* 8. Register Columns & Row Presentation */}
      <Section
        title="Register Columns & Row Presentation"
        description="Choose which columns the download register carries. The same set is reachable by right-clicking the column legends above the queue."
      >
        <div className="flex flex-wrap gap-1.5">
          {COLUMNS.map((column) => {
            const shown = visibleColumns.has(column.id);
            return (
              <button
                key={column.id}
                type="button"
                role="checkbox"
                aria-checked={shown}
                onClick={() => toggleColumn(column.id)}
                className={`focus-ring legend flex items-center gap-1.5 rounded-hw border px-2.5 py-1 text-[11px] transition-colors ${
                  shown
                    ? "border-route bg-route text-on-signal"
                    : "border-edge-strong text-ink-dim hover:bg-face hover:text-ink"
                }`}
              >
                <Check className={`size-3 ${shown ? "" : "invisible"}`} />
                {column.menuLabel}
              </button>
            );
          })}
        </div>

        <div className="mt-4 flex items-center justify-between gap-4 border-t border-edge pt-3">
          <p className="text-[12px] text-ink-dim">
            {visibleColumns.size} of {COLUMNS.length} columns shown
            {DEFAULT_COLUMNS.length === visibleColumns.size &&
            DEFAULT_COLUMNS.every((id) => visibleColumns.has(id))
              ? " (default set)"
              : ""}
          </p>
          <Button tone="ghost" onClick={resetColumns}>
            <span className="flex items-center gap-1.5">
              <RotateCcw className="size-3" />
              Reset Columns
            </span>
          </Button>
        </div>

        <div className="mt-4 divide-y divide-edge border-t border-edge">
          <ToggleRow
            title="Relative Timestamps"
            description="Display timestamps as elapsed relative time ('3 minutes ago') instead of absolute dates."
            checked={settings.telemetry.dateFormat === "relative"}
            onChange={() => toggleTelemetry("dateFormat")}
          />
          <ToggleRow
            title="Speed in Mbps (megabits)"
            description="Show download speed as Mbps (network convention, × 8÷ 1 000 000) instead of MB/s (binary bytes per second). The header throughput and rate column both follow this setting."
            checked={(settings.telemetry.speedUnit ?? "MBps") === "Mbps"}
            onChange={() => toggleTelemetry("speedUnit")}
          />
        </div>
      </Section>

      {/* 7. Reset Appearance & Theme */}
      <Section
        id="section-reset-appearance"
        title="Reset Appearance & Theme"
        description="Revert all appearance customizations, theme presets, colors, scaling, and telemetry options back to default Classic Dispatch."
      >
        <div className="flex items-center justify-between gap-4 border border-edge bg-face/50 p-4">
          <div>
            <p className="text-[13px] font-medium text-ink">Restore Default Theme & Appearance</p>
            <p className="mt-0.5 text-[12px] text-ink-dim">
              This will clear custom palette overrides and return to the default Classic Dispatch theme with factory appearance geometry.
            </p>
          </div>
          <Button
            tone="danger"
            onClick={() => {
              if (window.confirm("Are you sure you want to reset all appearance customizations and return to the default theme?")) {
                resetDefaults();
              }
            }}
          >
            <span className="flex items-center gap-1.5">
              <RotateCcw className="size-3.5" />
              Reset Appearance & Theme
            </span>
          </Button>
        </div>
      </Section>

      {/* Import Modal */}
      {showImportModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs modal-backdrop">
          <div className="plate modal-plate glass-panel flex w-full max-w-lg flex-col gap-4 rounded-hw-modal p-6 shadow-2xl">
            <div className="flex items-center justify-between border-b border-edge pb-2">
              <Legend className="text-[12px] text-ink">Import Custom Theme JSON</Legend>
              <button
                type="button"
                onClick={() => {
                  setShowImportModal(false);
                  setImportStatus(null);
                }}
                className="focus-ring text-ink-faint hover:text-ink text-sm"
              >
                ✕
              </button>
            </div>

            <p className="text-[12px] text-ink-dim">
              Paste valid Zoltraak theme JSON exported from another machine or community preset.
            </p>

            <textarea
              value={jsonInput}
              onChange={(e) => setJsonInput(e.target.value)}
              rows={8}
              autoComplete="off"
              spellCheck={false}
              placeholder={`{\n  "colors": {\n    "panel": "#0c0b08",\n    "route": "#f59e0b",\n    ...\n  }\n}`}
              className="describer recess w-full rounded-hw p-3 text-[11px] text-ink outline-none"
            />

            {importStatus && (
              <p
                className={`text-[12px] ${
                  importStatus.startsWith("Error") ? "text-danger" : "text-clear"
                }`}
              >
                {importStatus}
              </p>
            )}

            <div className="flex items-center justify-end gap-2 pt-2">
              <Button
                tone="ghost"
                onClick={() => {
                  setShowImportModal(false);
                  setImportStatus(null);
                }}
              >
                Cancel
              </Button>
              <Button onClick={handleImportSubmit}>Apply Theme</Button>
            </div>
          </div>
        </div>
      )}

      {/* Liquid Glass Inspection & Test Chamber Modal */}
      {showGlassModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 modal-backdrop animate-mechanical-settle">
          <div className="modal-plate glass-panel liquid-glass relative flex w-full max-w-xl flex-col gap-4 rounded-hw-modal p-6 shadow-2xl">
            {/* Header */}
            <div className="relative z-10 flex items-center justify-between border-b border-white/20 pb-3">
              <div className="flex items-center gap-2.5">
                <span className="size-2 rounded-full bg-route animate-pulse" />
                <Legend className="text-[13px] font-bold tracking-wider text-ink">
                  Liquid Glass Chamber
                </Legend>
                <span className="rounded-full border border-white/40 bg-white/10 px-2 py-0.5 text-[11px] font-mono uppercase text-ink">
                  Optical Refraction
                </span>
                <span className="inline-flex items-center gap-1 rounded-full border border-caution/40 bg-caution/15 px-2 py-0.5 text-[11px] font-mono font-bold uppercase text-caution">
                  <span className="size-1 rounded-full bg-caution animate-pulse" />
                  Beta
                </span>
              </div>
              <button
                type="button"
                onClick={() => setShowGlassModal(false)}
                className="focus-ring text-ink-dim hover:text-ink text-sm rounded-full size-7 flex items-center justify-center hover:bg-white/20 transition-colors"
                title="Close inspection"
              >
                ✕
              </button>
            </div>

            {/* Interactive Liquid Glass Specimen Stage */}
            <div className="relative z-10 flex flex-col items-center justify-center overflow-hidden rounded-hw-modal border border-white/20 p-5 shadow-inner">
              {/* Vibrant photographic/ambient scene background underneath the glass */}
              <div
                className="absolute inset-0 select-none"
                style={{
                  background:
                    "radial-gradient(circle at 35% 25%, #38bdf8 0%, #0284c7 30%, #0f172a 75%), linear-gradient(135deg, #f43f5e 0%, #a855f7 45%, #3b82f6 85%)",
                }}
              >
                {/* Visual texture patterns behind the glass to emphasize refraction */}
                <div className="absolute inset-0 opacity-25" style={{
                  backgroundImage: "repeating-linear-gradient(45deg, #fff 0, #fff 2px, transparent 0, transparent 16px)",
                }} />
                <div className="absolute bottom-2 left-3 text-[11px] font-mono text-white/40 tracking-wider">
                  BACKGROUND REFRACTION SCENE // HIGH VIBRANCY
                </div>
              </div>

              {/* The Liquid Glass Specimen Block */}
              <div
                role="region"
                aria-label="Interactive Liquid Glass specimen"
                onClick={(e) => {
                  const rect = e.currentTarget.getBoundingClientRect();
                  const x = Math.max(15, Math.min(85, Math.round(((e.clientX - rect.left) / rect.width) * 100)));
                  const y = Math.max(25, Math.min(80, Math.round(((e.clientY - rect.top) / rect.height) * 100)));
                  setGlassAnchor({ x, y });
                }}
                className="liquid-glass relative flex h-48 w-48 cursor-crosshair flex-col justify-between rounded-hw-modal p-3.5 shadow-2xl transition-all hover:scale-[1.02] active:scale-[0.99]"
              >
                <div className="relative z-10 flex items-center justify-between">
                  <span className="text-[11px] font-mono uppercase tracking-widest text-white/90 drop-shadow-sm font-semibold">
                    Liquid Glass
                  </span>
                  <span className="rounded-full border border-white/40 bg-white/20 px-1.5 py-0.5 text-[11px] font-mono font-bold text-white shadow-xs">
                    {Math.round(8 + (settings.effects.glassIntensity ?? 60) * 0.28)}px BLUR
                  </span>
                </div>

                {/* The Signature Liquid Glass Interactive Target Handle */}
                <div
                  className="pointer-events-none absolute transition-all duration-150"
                  style={{
                    left: `${glassAnchor.x}%`,
                    top: `${glassAnchor.y}%`,
                    transform: "translate(-50%, -50%)",
                  }}
                >
                  {/* Outer pulsating blue target pip with white ring & black center */}
                  <div className="relative flex size-7 items-center justify-center rounded-full border-2 border-white bg-blue-500 shadow-xl">
                    <div className="size-2 rounded-full bg-black ring-1 ring-white/60" />
                  </div>

                  {/* Vertical connector stem */}
                  <div className="mx-auto h-7 w-0.5 bg-white shadow-sm" />

                  {/* Diamond node handle */}
                  <div className="mx-auto -mt-4 size-2 rotate-45 border border-black/30 bg-white shadow-sm" />

                  {/* Bottom terminal endpoint pip */}
                  <div className="mx-auto mt-3.5 size-2 rounded-full border border-black/30 bg-white shadow-sm" />
                </div>

                <div className="relative z-10 flex items-end justify-between">
                  <span className="text-[11px] font-mono text-white/80 drop-shadow-sm">
                    {glassAnchor.x}% × {glassAnchor.y}%
                  </span>
                  <span className="text-[11px] font-mono uppercase tracking-wider text-pink-200/90 font-semibold drop-shadow-sm">
                    Prism Caustics
                  </span>
                </div>
              </div>
            </div>

            {/* Live Interactive Controls */}
            <div className="relative z-10 flex flex-col gap-3 rounded-hw-modal border border-white/25 bg-face/70 p-3.5 backdrop-blur-md">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <p className="text-[12.5px] font-semibold text-ink">Liquid Glass Physics Engine</p>
                  <p className="text-[11px] text-ink-dim">
                    Choose between dynamic liquid caustics and standard neutral matte frost
                  </p>
                </div>
                <SlideGroup className="flex overflow-hidden rounded-hw-xl border border-edge-strong bg-face">
                  <button
                    type="button"
                    onClick={() => setGlassStyle("liquid")}
                    className={`focus-ring px-3 py-1 text-[11px] font-mono transition-colors ${
                      (settings.effects.glassStyle ?? "liquid") === "liquid"
                        ? "bg-route text-on-signal font-semibold"
                        : "text-ink-dim hover:text-ink"
                    }`}
                  >
                    💧 Liquid Glass
                  </button>
                  <button
                    type="button"
                    data-active={settings.effects.glassStyle === "frosted"}
                    onClick={() => setGlassStyle("frosted")}
                    className={`focus-ring border-l border-edge px-3 py-1 text-[11px] font-mono transition-colors ${
                      settings.effects.glassStyle === "frosted"
                        ? "text-on-signal font-semibold"
                        : "text-ink-dim hover:text-ink"
                    }`}
                  >
                    ❄ Matte Frost
                  </button>
                </SlideGroup>
              </div>

              {/* Blur & Refraction Radius Slider */}
              <div className="flex items-center gap-3 border-t border-edge/60 pt-2.5">
                <span className="min-w-[110px] text-[11px] font-mono uppercase tracking-wider text-ink-faint">
                  Refraction & Blur
                </span>
                <input
                  type="range"
                  min="10"
                  max="100"
                  step="5"
                  value={settings.effects.glassIntensity ?? 60}
                  disabled={!settings.effects.glass}
                  onChange={(e) => setEffectIntensity("glassIntensity", Number(e.target.value))}
                  className="accent-route h-1.5 flex-1 cursor-pointer rounded-hw bg-recess disabled:cursor-not-allowed"
                />
                <span className="min-w-[70px] text-right font-mono text-[11px] font-semibold text-ink">
                  {Math.round(8 + (settings.effects.glassIntensity ?? 60) * 0.28)}px ({settings.effects.glassIntensity ?? 60}%)
                </span>
              </div>
            </div>

            {/* Live Optical Telemetry Gauges */}
            <div className="relative z-10 grid grid-cols-2 gap-2 text-[11px] sm:grid-cols-4">
              <div className="rounded-hw-panel border border-white/20 bg-recess/75 p-2.5 backdrop-blur-xs">
                <span className="font-mono text-[11px] text-ink-faint uppercase">REFRACTIVE INDEX</span>
                <p className="mt-0.5 font-mono text-[11px] font-semibold text-clear">
                  n = 1.52 (Crown Glass)
                </p>
              </div>
              <div className="rounded-hw-panel border border-white/20 bg-recess/75 p-2.5 backdrop-blur-xs">
                <span className="font-mono text-[11px] text-ink-faint uppercase">SPECULAR MENISCUS</span>
                <p className="mt-0.5 font-mono text-[11px] font-semibold text-route">
                  Polarized Lip Bead
                </p>
              </div>
              <div className="rounded-hw-panel border border-white/20 bg-recess/75 p-2.5 backdrop-blur-xs">
                <span className="font-mono text-[11px] text-ink-faint uppercase">PRISM DISPERSION</span>
                <p className="mt-0.5 font-mono text-[11px] font-semibold text-pink-500">
                  RGB Caustics Edge
                </p>
              </div>
              <div className="rounded-hw-panel border border-white/20 bg-recess/75 p-2.5 backdrop-blur-xs">
                <span className="font-mono text-[11px] text-ink-faint uppercase">VIVID SATURATION</span>
                <p className="mt-0.5 font-mono text-[11px] font-semibold text-ink">
                  {Math.round(130 + (settings.effects.glassIntensity ?? 60) * 0.7)}%
                </p>
              </div>
            </div>

            {/* Simultaneous Atmosphere Combination Toggles */}
            <div className="relative z-10 rounded-hw-panel border border-white/20 bg-face/50 p-2.5">
              <span className="mb-1.5 block font-mono text-[11px] uppercase tracking-wider text-ink-faint">
                Combine Atmosphere Overlays with Liquid Glass:
              </span>
              <div className="flex flex-wrap gap-1.5">
                <button
                  type="button"
                  onClick={() => setEffect("bloom", !settings.effects.bloom)}
                  className={`focus-ring legend rounded-hw border px-2 py-0.5 text-[11px] transition-colors ${
                    settings.effects.bloom
                      ? "border-clear bg-clear text-on-signal font-semibold shadow-xs"
                      : "border-edge bg-recess text-ink-dim hover:text-ink"
                  }`}
                >
                  Phosphor Bloom: {settings.effects.bloom ? "ON" : "OFF"}
                </button>
                <button
                  type="button"
                  onClick={() => setEffect("grid", !settings.effects.grid)}
                  className={`focus-ring legend rounded-hw border px-2 py-0.5 text-[11px] transition-colors ${
                    settings.effects.grid
                      ? "border-route bg-route text-on-signal font-semibold shadow-xs"
                      : "border-edge bg-recess text-ink-dim hover:text-ink"
                  }`}
                >
                  Blueprint Grid: {settings.effects.grid ? "ON" : "OFF"}
                </button>
                <button
                  type="button"
                  onClick={() => setEffect("noise", !settings.effects.noise)}
                  className={`focus-ring legend rounded-hw border px-2 py-0.5 text-[11px] transition-colors ${
                    settings.effects.noise
                      ? "border-caution bg-caution text-on-signal font-semibold shadow-xs"
                      : "border-edge bg-recess text-ink-dim hover:text-ink"
                  }`}
                >
                  Analog Noise: {settings.effects.noise ? `ON (${settings.effects.noisePlacement ?? "overlay"})` : "OFF"}
                </button>
              </div>
            </div>

            {/* Footer */}
            <div className="relative z-10 flex items-center justify-end gap-2 border-t border-white/20 pt-2">
              <Button onClick={() => setShowGlassModal(false)}>Close Chamber</Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
