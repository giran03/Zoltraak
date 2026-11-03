import { useEffect, useRef, useState } from "react";
import { Button, Switch } from "./Controls";
import {
  CUSTOM_FONT_FAMILY,
  FONT_ACCEPT,
  MAX_FONT_BYTES,
  checkFontFile,
  deleteCustomFontData,
  fontDisplayName,
  invalidateCustomFont,
  loadFontFace,
  readCustomFontData,
  saveCustomFontData,
  type CustomFontMeta,
} from "../lib/customFont";
import { TYPEFACE_PRESETS } from "../lib/typefaces";
import type { FontMode } from "../lib/useAppearance";

const PREVIEW_FAMILY = "ZoltraakCustomPreview";
const SAMPLE_TEXT = "Quick brown fox";
const SAMPLE_FIGURES = "0123456789  42.7 MB/s";
const DEFAULT_SANS = TYPEFACE_PRESETS[0]?.sans ?? "sans-serif";
const DEFAULT_MONO = TYPEFACE_PRESETS[0]?.mono ?? "monospace";

function megabytes(bytes: number) {
  return bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** One option: its name, a line of text and a line of figures in its own faces, and what is behind it. */
function TypefaceCard({ title, note, sans, mono, selected, onSelect }: {
  title: string;
  note: string;
  sans: string;
  mono: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      onClick={onSelect}
      className={`focus-ring flex min-w-0 flex-col gap-1.5 rounded-hw border p-3 text-left transition-colors ${
        selected ? "border-route bg-route/10" : "border-edge-strong bg-face hover:bg-recess"
      }`}
    >
      <span className="flex items-center justify-between gap-2">
        <span className="legend truncate text-[11px] text-ink">{title}</span>
        {selected && <span className="legend shrink-0 text-[11px] text-route">In use</span>}
      </span>
      <span className="truncate text-[15px] leading-tight text-ink" style={{ fontFamily: sans }}>{SAMPLE_TEXT}</span>
      <span className="truncate text-[12px] text-ink-dim" style={{ fontFamily: mono }}>{SAMPLE_FIGURES}</span>
      <span className="text-[11px] leading-snug text-ink-faint">{note}</span>
    </button>
  );
}

/**
 * Typeface Character: the bundled presets, each previewed in the faces it really
 * applies, plus one uploaded font. The uploaded file is checked, parsed by the
 * browser and kept on this machine; it never leaves it.
 */
export function TypefacePicker({ mode, customFont, onMode, onCustomFont }: {
  mode: FontMode;
  customFont: CustomFontMeta | null;
  onMode: (mode: FontMode) => void;
  onCustomFont: (font: CustomFontMeta | null) => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [previewReady, setPreviewReady] = useState(false);

  // Load the upload under its own family so its card can preview it even while
  // another typeface is the one in use.
  useEffect(() => {
    if (!customFont) {
      setPreviewReady(false);
      return;
    }
    let cancelled = false;
    let face: FontFace | null = null;
    void (async () => {
      try {
        const data = await readCustomFontData();
        if (!data || cancelled) return;
        face = await loadFontFace(data, PREVIEW_FAMILY);
        if (cancelled) return;
        document.fonts.add(face);
        setPreviewReady(true);
      } catch {
        if (!cancelled) setPreviewReady(false);
      }
    })();
    return () => {
      cancelled = true;
      if (face) document.fonts.delete(face);
    };
  }, [customFont?.fileName, customFont?.bytes]);

  async function accept(file: File) {
    setError(null);
    setBusy(true);
    try {
      const data = await file.arrayBuffer();
      const problem = checkFontFile(file.name, file.size, new Uint8Array(data, 0, Math.min(4, data.byteLength)));
      if (problem) throw new Error(problem);
      // The browser has the final say on whether the bytes are a usable font.
      try {
        await loadFontFace(data, `${CUSTOM_FONT_FAMILY}Check`);
      } catch {
        throw new Error("That file could not be read as a font.");
      }
      await saveCustomFontData(data);
      invalidateCustomFont();
      onCustomFont({ name: fontDisplayName(file.name), fileName: file.name, bytes: file.size, applyToMono: customFont?.applyToMono ?? false });
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : "That font could not be added.");
    } finally {
      setBusy(false);
      if (input.current) input.current.value = "";
    }
  }

  async function remove() {
    setError(null);
    try {
      await deleteCustomFontData();
    } catch {
      // The setting is what selects the font; a file that cannot be deleted is harmless.
    }
    invalidateCustomFont();
    onCustomFont(null);
  }

  const previewSans = previewReady ? `"${PREVIEW_FAMILY}", ${DEFAULT_SANS}` : DEFAULT_SANS;
  const previewMono = previewReady && customFont?.applyToMono ? `"${PREVIEW_FAMILY}", ${DEFAULT_MONO}` : DEFAULT_MONO;

  return (
    <div className="flex flex-col gap-3">
      <div role="radiogroup" aria-label="Typeface" className="grid grid-cols-1 gap-2 sm:grid-cols-2 xl:grid-cols-3">
        {TYPEFACE_PRESETS.map((preset) => (
          <TypefaceCard
            key={preset.id}
            title={preset.label}
            note={preset.note}
            sans={preset.sans}
            mono={preset.mono}
            selected={mode === preset.id}
            onSelect={() => onMode(preset.id)}
          />
        ))}
        {customFont ? (
          <TypefaceCard
            title={customFont.name}
            note={`Your upload · ${customFont.fileName || "font file"}${customFont.bytes ? ` · ${megabytes(customFont.bytes)}` : ""}`}
            sans={previewSans}
            mono={previewMono}
            selected={mode === "custom"}
            onSelect={() => onMode("custom")}
          />
        ) : (
          <button
            type="button"
            disabled={busy}
            onClick={() => input.current?.click()}
            className="focus-ring flex min-w-0 flex-col items-start justify-center gap-1.5 rounded-hw border border-dashed border-edge-strong p-3 text-left text-ink-dim transition-colors hover:bg-recess hover:text-ink disabled:opacity-60"
          >
            <span className="legend text-[11px] text-ink">{busy ? "Adding…" : "Upload your own"}</span>
            <span className="text-[11px] leading-snug text-ink-faint">
              WOFF2, WOFF, TTF or OTF, up to {MAX_FONT_BYTES / 1024 / 1024} MB. It stays on this computer.
            </span>
          </button>
        )}
      </div>

      {customFont && (
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
          <Button tone="ghost" disabled={busy} onClick={() => input.current?.click()}>{busy ? "Adding…" : "Replace file"}</Button>
          <Button tone="ghost" onClick={() => void remove()}>Remove</Button>
          <div className="flex items-center gap-2">
            <Switch
              label="Also use it for figures and code"
              checked={customFont.applyToMono}
              onChange={(applyToMono) => onCustomFont({ ...customFont, applyToMono })}
            />
            <span className="text-[12px] text-ink-dim">Also use it for figures and code</span>
          </div>
        </div>
      )}

      {error && <p role="alert" className="text-[12px] text-danger">{error}</p>}

      <input
        ref={input}
        type="file"
        accept={FONT_ACCEPT}
        hidden
        onChange={(event) => {
          const file = event.target.files?.[0];
          if (file) void accept(file);
        }}
      />
    </div>
  );
}
