import { DEFAULT_DARK_COLORS, normalizeIconPalette, resolvedIconPalette, resolvedTheme, useAppearance } from "../lib/useAppearance";
import { refreshAppIcon, useIconStatus, useSystemDark } from "../lib/AppIconSync";
import emblem from "../assets/zoltraak-emblem.png";
import { Button, Section, Switch } from "./Controls";

function contrast(a: string, b: string) {
  const luminance = (hex: string) => hex.slice(1).match(/../g)!.map(v => parseInt(v, 16) / 255).map(v => v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4).reduce((sum, v, i) => sum + v * ([.2126, .7152, .0722][i] ?? 0), 0);
  const x = luminance(a), y = luminance(b);
  return (Math.max(x, y) + .05) / (Math.min(x, y) + .05);
}
export function AppIconSettings() {
  const { settings, update } = useAppearance();
  const dark = useSystemDark(), palette = resolvedIconPalette(settings, dark), status = useIconStatus();
  const native = "__TAURI_INTERNALS__" in window;
  const name = palette.key.startsWith("preset:") ? "this preset" : palette.key === "liquid-glass" ? "Liquid Glass" : `${palette.key} theme`;
  const canvas = resolvedTheme(settings, dark).colors.panel;
  const change = (patch: Partial<typeof palette>) => update(prev => ({ ...prev, iconPalettes: { ...prev.iconPalettes, [palette.key]: { useAppColors: palette.useAppColors, foreground: palette.foreground, ...normalizeIconPalette(prev.iconPalettes[palette.key] ?? (palette.key.startsWith("preset:") ? prev.userPresets.find(p => `preset:${p.id}` === palette.key)?.iconPalette : undefined)), ...patch } } }));
  // Scrubbing only redraws this small preview; publishing appearance settings
  // also redraws the rest of the app, writes storage, and schedules native work.
  const [draft, setDraft] = useState<string | null>(null);
  const pendingColor = useRef<(() => void) | null>(null);
  const colorTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const flushColor = useCallback(() => {
    if (colorTimer.current !== null) clearTimeout(colorTimer.current);
    colorTimer.current = null;
    const commit = pendingColor.current;
    pendingColor.current = null;
    commit?.();
  }, []);
  useEffect(() => { setDraft(null); }, [palette.key, palette.foreground, palette.useAppColors]);
  useEffect(() => {
    window.addEventListener("pagehide", flushColor);
    return () => { window.removeEventListener("pagehide", flushColor); flushColor(); };
  }, [flushColor]);
  const foreground = draft ?? palette.foreground;
  const previewColor = (value: string) => {
    setDraft(value);
    pendingColor.current = () => change({ foreground: value });
    if (colorTimer.current !== null) clearTimeout(colorTimer.current);
    colorTimer.current = setTimeout(flushColor, 300);
  };
  return <Section title="App icon" description="A transparent, full-size emblem with a color saved for each theme. System mode follows your light and dark icon colors.">
    <div className="flex items-center justify-between gap-4"><div><p className="text-[13px] text-ink">Sync app icon with theme</p><p className="mt-1 text-[12px] text-ink-dim">Updates the window, tray, and writable Zoltraak desktop and Start Menu shortcuts.</p></div><Switch label="Sync app icon with theme" checked={settings.syncAppIcon} onChange={syncAppIcon => update({ syncAppIcon })}/></div>
    <div className="mt-2 flex flex-col gap-5 sm:flex-row sm:items-start">
      <div className="flex shrink-0 items-end gap-3" aria-label="App icon preview">
        {[96, 32].map(size => <div key={size} role="img" aria-label={`Icon preview ${size} pixels`} style={{ width: size, height: size }}><div className="size-full" style={{ backgroundColor: foreground, WebkitMaskImage: `url(${emblem})`, WebkitMaskSize: "contain", WebkitMaskRepeat: "no-repeat", maskImage: `url(${emblem})`, maskSize: "contain", maskRepeat: "no-repeat", maskPosition: "center" }}/></div>)}
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        <div className="flex items-center justify-between gap-4"><div><p className="text-[13px] text-ink">Use Route Highlight color</p><p className="mt-1 text-[12px] text-ink-dim">Editing {name}. Follows Route Highlight; the background stays transparent.</p></div><Switch label="Use Route Highlight color for icon" checked={palette.useAppColors} onChange={useAppColors => change({ useAppColors })}/></div>
        <label className="flex min-w-0 flex-col gap-2 text-[12px] text-ink"><span>Icon foreground</span><span className="flex items-center gap-2"><input type="color" aria-label="Icon foreground" className="focus-ring h-8 w-10 shrink-0 cursor-pointer rounded-hw border border-edge bg-recess disabled:cursor-default disabled:opacity-50" disabled={palette.useAppColors} value={foreground} onInput={e => previewColor(e.currentTarget.value)} onChange={e => previewColor(e.target.value)} onBlur={flushColor}/><span className="font-mono text-[11px] text-ink-dim">{foreground.toUpperCase()}</span></span></label>
        {contrast(canvas, foreground) < 3 && <p role="status" className="text-[12px] text-caution">This color has low contrast against the app canvas. Check visibility against your desktop background too.</p>}
        {!settings.syncAppIcon && <p className="text-[12px] text-ink-dim">Sync is off. App icons use the default palette; this preview keeps your theme palette ready.</p>}
        <div className="flex flex-wrap gap-2"><Button tone="ghost" onClick={() => change({ useAppColors: true, foreground })}>Reset icon colors</Button><Button disabled={!native || status.busy} onClick={() => { flushColor(); void refreshAppIcon(settings.syncAppIcon ? foreground : DEFAULT_DARK_COLORS.route); }}>{status.busy ? "Updating icons…" : "Refresh desktop icon"}</Button></div>
        <p role="status" aria-live="polite" className={`text-[12px] ${status.error ? "text-caution" : "text-ink-dim"}`}>{!native ? "Desktop icon syncing is available in the installed app." : status.message || "Pinned taskbar icons may keep a Windows-cached image until repinned."}</p>
      </div>
    </div>
  </Section>;
}
import { useCallback, useEffect, useRef, useState } from "react";
