import { useEffect, useId, useRef, useState } from "react";
import { ConversionHelp, type ConversionHelpTopic } from "./ConversionHelp";
import { ConvertJobs } from "./ConvertJobs";
import { ConvertPreview } from "./ConvertPreview";
import { ConvertWatchRules } from "./ConvertWatchRules";
import { TrimCropEditor } from "./TrimCropEditor";
import { Button, ClearButton, ErrorStrip, Field, NumberInput, Section, SelectInput, Switch, TextInput } from "./Controls";
import { Close, Film, Frame, Note, Transfer } from "./Icons";
import { Legend } from "./Panel";
import { UnsupportedFilesModal } from "./UnsupportedFilesModal";
import type { ToastItem } from "./Toast";
import { engine, hasEngine, type ConversionFormat, type ConversionSettings } from "../lib/engine";
import { CONVERSION_FORMATS, CONVERSION_PRESET_LIBRARY, NAME_TOKENS, SAMPLE_RATES, adaptSettingsToFormat, conversionPreset, defaultConversionSettings, formatKind, nameTemplateError, outputFieldSupport, parseTimestamp, presetSupportsFormat, previewOutputName, supportsSampleRate } from "../lib/conversion";
import { basename } from "../lib/media";
import { judgeDrop, resolveDroppedPaths } from "../lib/fileDrop";
import { formatBytes, formatEta } from "../lib/format";
import { safeStorage } from "../lib/safeStorage";
import type { useConvert } from "../lib/useConvert";

type Converter = ReturnType<typeof useConvert>;

function matchesPreset(settings: ConversionSettings, preset: ConversionSettings): boolean {
  return (Object.keys(preset) as (keyof ConversionSettings)[]).every((key) => {
    if (Object.is(settings[key], preset[key])) return true;
    if (key === "crop" && settings.crop && preset.crop) {
      return (["x", "y", "width", "height"] as const).every((axis) => settings.crop![axis] === preset.crop![axis]);
    }
    return false;
  });
}

function ConversionToggle({ label, description, checked, onChange, disabled, help }: { label: string; description: string; checked: boolean; onChange: (value: boolean) => void; disabled?: boolean; help?: ConversionHelpTopic }) {
  const descriptionId = useId();
  return <div className={`flex items-start gap-3 ${disabled ? "opacity-55" : ""}`}>
    <Switch checked={checked} onChange={onChange} label={label} disabled={disabled} descriptionId={descriptionId} />
    <div className="min-w-0"><div className="flex items-center gap-1.5"><span className="text-[13px] text-ink">{label}</span>{help && <ConversionHelp topic={help} />}</div><p id={descriptionId} className="mt-1 text-[12px] leading-relaxed text-ink-dim">{description}</p></div>
  </div>;
}

function NumberField({ label, value, onChange, hint, help, disabled, min = 0, max = 16384, step = "any" }: {
  label: string; value: number | null; onChange: (value: number | null) => void;
  hint?: string; help?: ConversionHelpTopic; disabled?: boolean; min?: number; max?: number; step?: string;
}) {
  return <Field label={label} hint={hint} info={help && <ConversionHelp topic={help} />}><NumberInput ariaLabel={label} min={min} max={max} step={step === "any" ? "any" : Number(step)}
    disabled={disabled}
    value={value !== null && Number.isFinite(value) ? String(value) : ""}
    onChange={text => onChange(text === "" ? null : Number(text))} /></Field>;
}

function TimeField({ label, value, onChange, hint, disabled }: { label: string; value: number | null; onChange: (n: number | null) => void; hint?: string; disabled?: boolean }) {
  const [draft, setDraft] = useState(value === null ? "" : String(value));
  useEffect(() => { if (value === null || Number.isFinite(value)) setDraft(value === null ? "" : String(value)); }, [value]);
  const invalid = Number.isNaN(parseTimestamp(draft));
  return <Field label={label} hint={invalid ? "Use seconds or HH:MM:SS.mmm" : hint}>
    <TextInput value={draft} onChange={(text) => { setDraft(text); onChange(parseTimestamp(text)); }} ariaLabel={label} placeholder="00:00:00.000" disabled={disabled} mono />
  </Field>;
}

function SettingsFields({ convert }: { convert: Converter }) {
  const settings = convert.settings;
  const set = <K extends keyof ConversionSettings>(key: K, value: ConversionSettings[K]) => convert.setSettings((previous) => ({ ...previous, [key]: value }));
  const kind = formatKind(settings.format);
  const support = outputFieldSupport(settings);
  const selectedFile = convert.files.find((file) => file.path === convert.selection?.path) ?? convert.files[0];
  const tracks = selectedFile?.audioTracks ?? [];
  const stillSource = Boolean(selectedFile && !selectedFile.unprobed && selectedFile.durationSeconds === null && (convert.customized || convert.files.length === 1));
  return <>
    <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
      <Field label="Processing" info={<ConversionHelp topic="processing" />} hint="Choose Encode to apply quality or bitrate changes, including same-format conversions.">
        <SelectInput ariaLabel="Processing" value={settings.mode} onChange={(value) => set("mode", value)} options={[
          { value: "auto", label: "Automatic · copy when compatible" }, { value: "encode", label: "Encode · apply quality settings" }, { value: "copy", label: "Stream copy only", disabled: !support.streamCopy },
        ]} />
      </Field>
      <NumberField help="quality" disabled={!support.quality} label={settings.format === "png" ? "PNG compression effort (1–100)" : "Quality (1–100)"} value={settings.quality} onChange={(value) => set("quality", value ?? 80)} min={1} max={100} step="1" hint={settings.format === "png" ? "Compression effort changes size and speed; pixels remain lossless." : "Higher values favor quality; the exact encoder setting depends on format."} />
      <NumberField disabled={!support.targetSize} label="Target file size (MB)" value={settings.targetSizeMb} onChange={(value) => set("targetSizeMb", value)} min={0.1} max={100000} hint="Optional. Picks the bitrate from the clip length so the result lands near this size; it can land slightly above or below. Empty uses the quality setting." />
      <TimeField disabled={stillSource} label={kind === "Image" ? "Frame timestamp" : "Start time"} value={settings.startSeconds} onChange={(value) => set("startSeconds", value ?? 0)} />
      <TimeField disabled={!support.endTime || stillSource} label="End time" value={settings.endSeconds} onChange={(value) => set("endSeconds", value)} hint="Empty uses the end of the source." />
      <NumberField help="dimensions" disabled={!support.dimensions} label="Maximum width (px)" value={settings.width} onChange={(value) => set("width", value)} min={1} step="1" hint="Leave dimensions empty to keep the original. Aspect ratio is preserved." />
      <NumberField help="dimensions" disabled={!support.dimensions} label="Maximum height (px)" value={settings.height} onChange={(value) => set("height", value)} min={1} step="1" />
      <NumberField help="fps" disabled={!support.fps} label="Frame rate (fps)" value={settings.fps} onChange={(value) => set("fps", value)} min={1} max={120} hint="Empty preserves source timing." />
      <NumberField disabled={!support.duration} label="Clip duration (seconds)" value={settings.durationSeconds} onChange={(value) => set("durationSeconds", value)} min={0.001} max={86400} hint="Required for a still image → clip. End time takes precedence." />
      <NumberField help="bitrate" disabled={!support.audioBitrate} label="Audio bitrate (kbps)" value={settings.audioBitrate} onChange={(value) => set("audioBitrate", value ?? 192)} min={6} max={512} step="1" hint="Applied when the audio is encoded." />
      <Field label="Audio track" hint="Tracks shown for the selected file; each file is validated separately.">
        <SelectInput ariaLabel="Audio track" disabled={!support.audioTrack} value={String(settings.audioTrack)} onChange={(value) => set("audioTrack", Number(value))} options={tracks.length ? tracks.map((track) => ({ value: String(track.index), label: `${track.index + 1} · ${track.language ?? "und"} · ${track.codec} · ${track.channels ?? "?"} ch` })) : [{ value: "0", label: "First audio track" }]} />
      </Field>
    </div>
    <details className="border-t border-edge pt-3">
      <summary className="focus-ring cursor-pointer text-[12px] text-ink-dim">Advanced settings</summary>
      <div className="mt-4 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <Field label="Background color" hint="Used when the output cannot preserve transparency."><TextInput disabled={!support.background} value={settings.background} onChange={(value) => set("background", value)} ariaLabel="Background color" placeholder="#ffffff" mono /></Field>
        <NumberField disabled={!support.loopCount} label="Loop count" value={settings.loopCount} onChange={(value) => set("loopCount", value ?? 0)} max={65535} step="1" hint="0 repeats forever; 1 plays once." />
        <Field label="GIF dithering" info={<ConversionHelp topic="dithering" />} hint="Blends GIF colors using dots. Open the info button to compare the options."><SelectInput ariaLabel="GIF dithering" disabled={!support.dither} value={settings.dither} onChange={(value) => set("dither", value)} options={[{ value: "sierra2_4a", label: "Smooth · Sierra" }, { value: "bayer", label: "Ordered · Bayer" }, { value: "none", label: "None" }]} /></Field>
        <Field label="Sample rate" info={<ConversionHelp topic="sampleRate" />}><SelectInput ariaLabel="Sample rate" disabled={!support.audioProcessing} value={String(settings.sampleRate ?? "")} onChange={(value) => set("sampleRate", value ? Number(value) : null)} options={[{ value: "", label: "Keep source" }, ...SAMPLE_RATES.map((rate) => ({ value: String(rate), label: `${rate} Hz`, disabled: !supportsSampleRate(settings.format, rate) }))]} /></Field>
        <Field label="Channels" info={<ConversionHelp topic="channels" />}><SelectInput ariaLabel="Channels" disabled={!support.audioProcessing} value={String(settings.channels ?? "")} onChange={(value) => set("channels", value ? Number(value) : null)} options={[{ value: "", label: "Keep source" }, { value: "1", label: "Mono" }, { value: "2", label: "Stereo" }]} /></Field>
        <NumberField disabled={!support.audioProcessing} label="Fade in (seconds)" value={settings.fadeIn} onChange={(value) => set("fadeIn", value ?? 0)} max={3600} />
        <NumberField disabled={!support.audioProcessing} label="Fade out (seconds)" value={settings.fadeOut} onChange={(value) => set("fadeOut", value ?? 0)} max={3600} />
        <NumberField disabled={!support.loudnessTargets} label="Loudness target (LUFS)" value={settings.loudnessLufs ?? -16} onChange={(value) => set("loudnessLufs", value ?? -16)} min={-70} max={-5} />
        <NumberField disabled={!support.loudnessTargets} label="True peak limit (dBTP)" value={settings.truePeakDb ?? -1.5} onChange={(value) => set("truePeakDb", value ?? -1.5)} min={-9} max={0} />
        <ConversionToggle disabled={!support.losslessWebp} checked={settings.lossless} onChange={(value) => set("lossless", value)} label="Lossless WebP encoding" description="Preserve decoded pixels without lossy compression. Files may be larger." help="quality" />
        <ConversionToggle disabled={!support.audioProcessing} checked={settings.normalize} onChange={(value) => set("normalize", value)} label="Normalize loudness" description="Adjust listening level to the LUFS target and true-peak limit below." help="loudness" />
        <ConversionToggle disabled={!support.loudnessTargets} checked={settings.normalizeTwoPass ?? false} onChange={(value) => set("normalizeTwoPass", value)} label="Measure loudness first" description="Analyze the selected audio, then normalize with measured values. Takes two passes; silence uses a single-pass fallback." />
        <ConversionToggle disabled={!support.mute} checked={settings.mute} onChange={(value) => set("mute", value)} label="Remove audio" description="Save the video without its soundtrack." />
        <ConversionToggle checked={settings.stripMetadata} onChange={(value) => set("stripMetadata", value)} label="Remove metadata" description="On: omit copied titles, artist names, albums, and comments. Off: keep fields supported by the output format." help="metadata" />
      </div>
    </details>
  </>;
}

export function ConvertView({ convert, onAddToast }: { convert: Converter; onAddToast?: (toast: ToastItem) => void }) {
  const root = useRef<HTMLDivElement>(null);
  const source = useRef<HTMLDivElement>(null);
  const selectedFile = convert.files.find((file) => file.path === convert.selection?.path) ?? convert.files[0];
  const kind = formatKind(convert.settings.format);
  const [managingFiles, setManagingFiles] = useState(false);
  useEffect(() => {
    if (convert.selection && convert.selection.focus !== false) setManagingFiles(false);
    if (convert.selection && convert.selection.focus !== false && source.current && source.current.getClientRects().length > 0) {
      source.current.focus({ preventScroll: true });
      root.current?.scrollIntoView({ block: "start" });
    }
  }, [convert.selection]);
  const [pathDraft, setPathDraft] = useState("");
  // Files dropped on the Files area: what was refused, and why nothing was added.
  const [dragging, setDragging] = useState(false);
  const dragDepth = useRef(0);
  const [dropIssue, setDropIssue] = useState<{ names: string[]; added: number } | null>(null);
  const [dropNotice, setDropNotice] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installError, setInstallError] = useState<string | null>(null);
  const [presetName, setPresetName] = useState("");
  const [presets, setPresets] = useState<Record<string, ConversionSettings>>(() => {
    try { const value = JSON.parse(safeStorage.getItem("zoltraak.convert.presets.v1") ?? "{}"); return value && typeof value === "object" && !Array.isArray(value) ? value : {}; } catch { return {}; }
  });
  const [selectedPresets, setSelectedPresets] = useState<Record<string, { id: string; settings: ConversionSettings }>>(() => ({
    shared: { id: "", settings: defaultConversionSettings() },
  }));
  const presetScope = convert.customized && selectedFile ? selectedFile.path : "shared";
  const selectedPreset = selectedPresets[presetScope] ?? selectedPresets.shared ?? { id: "", settings: defaultConversionSettings() };
  const presetValue = matchesPreset(convert.settings, selectedPreset.settings) ? selectedPreset.id : "custom";
  function setIndividualSettings(on: boolean) {
    if (on && selectedFile) {
      setSelectedPresets((previous) => ({ ...previous, [selectedFile.path]: { id: presetValue, settings: convert.settings } }));
    }
    convert.setCustomized(on);
  }
  function applyPreset(name: string) {
    if (!name || name === "custom") return;
    const settings = { ...(name.startsWith("saved:") ? { ...defaultConversionSettings(), ...presets[name.slice(6)] } : conversionPreset(name, convert.settings.format)), nameTemplate: convert.settings.nameTemplate };
    convert.setSettings(settings);
    setSelectedPresets((previous) => ({ ...previous, [presetScope]: { id: name, settings } }));
  }
  function savePreset() {
    const name = presetName.trim();
    if (!name) return;
    const next = { ...presets, [name]: convert.settings };
    setPresets(next);
    safeStorage.setItem("zoltraak.convert.presets.v1", JSON.stringify(next));
    setSelectedPresets((previous) => ({ ...previous, [presetScope]: { id: `saved:${name}`, settings: convert.settings } }));
    setPresetName("");
  }
  const available = convert.capabilities?.formats;
  const nameError = nameTemplateError(convert.settings.nameTemplate);
  const compatible = convert.plans.filter((row) => row.plan && !row.error).length;
  const ready = !convert.planning && convert.plans.length === convert.files.length && compatible > 0 && available?.includes(convert.settings.format) && convert.capabilities?.ffprobe;
  async function addPaths() {
    const count = await convert.addPaths(pathDraft.split(/\r?\n/).map((path) => path.trim().replace(/^"|"$/g, "")));
    if (count > 0) { setPathDraft(""); setManagingFiles(false); }
  }
  const carriesFiles = (event: React.DragEvent) => Array.from(event.dataTransfer.types).includes("Files");
  const dropProps = {
    onDragEnter: (event: React.DragEvent) => {
      if (!carriesFiles(event)) return;
      event.preventDefault();
      dragDepth.current += 1;
      setDragging(true);
    },
    onDragOver: (event: React.DragEvent) => {
      if (!carriesFiles(event)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "copy";
    },
    onDragLeave: (event: React.DragEvent) => {
      if (!carriesFiles(event)) return;
      // dragleave also fires crossing between child elements, so count depth.
      dragDepth.current = Math.max(0, dragDepth.current - 1);
      if (dragDepth.current === 0) setDragging(false);
    },
    onDrop: (event: React.DragEvent) => {
      if (!carriesFiles(event)) return;
      event.preventDefault();
      dragDepth.current = 0;
      setDragging(false);
      void acceptDrop(event.dataTransfer);
    },
  };
  /** Add the supported files and folders that were dropped; say so about the rest. */
  async function acceptDrop(data: DataTransfer) {
    // The drop's contents are only readable during the event, so take them now.
    const dropped = Array.from(data.items)
      .filter((item) => item.kind === "file")
      .flatMap((item) => {
        const file = item.getAsFile();
        return file ? [{ file, name: file.name, isDirectory: Boolean(item.webkitGetAsEntry?.()?.isDirectory) }] : [];
      });
    if (dropped.length === 0) return;
    setDropNotice(null);
    const verdict = judgeDrop(dropped);
    let added = 0;
    if (verdict.accepted.length > 0) {
      const paths = await resolveDroppedPaths(verdict.accepted.map((entry) => entry.file));
      if (paths === null) setDropNotice("Dropping files is not available in this window. Use Choose files, or paste the paths below.");
      else if (paths.length === 0) setDropNotice("The location of the dropped files could not be read. Use Choose files instead.");
      else {
        added = await convert.addPaths(paths);
        if (added > 0) setManagingFiles(false);
      }
    }
    if (verdict.unsupported.length > 0) setDropIssue({ names: verdict.unsupported, added });
  }
  async function browseFiles() {
    const count = await convert.browse();
    if (count > 0) setManagingFiles(false);
  }
  async function install() {
    setInstalling(true); setInstallError(null);
    try { await engine.installFfmpeg(); await convert.refreshCapabilities(); }
    catch (e) { setInstallError(e instanceof Error ? e.message : String(e)); }
    finally { setInstalling(false); }
  }
  async function run() {
    const count = await convert.start();
    if (count) onAddToast?.({ id: `convert-${Date.now()}`, type: "info", title: `${count} ${count === 1 ? "conversion" : "conversions"} queued`, message: "Each job uses the settings shown when it was queued." });
  }
  return <div ref={root} className="conversion-view mx-auto flex w-full max-w-4xl flex-col gap-8 px-6 py-7">
    {selectedFile && <Section title="Source" aside={convert.files.length > 1 ? <Legend>{convert.files.length} files</Legend> : undefined}>
      <div ref={source} tabIndex={-1} role="group" aria-label={`Conversion input ${basename(selectedFile.path)}`} data-conversion-source-path={selectedFile.path} className="focus-ring flex min-w-0 items-start gap-3 border-l-2 border-route pl-4 py-1">
        <span className="mt-1 flex size-9 shrink-0 items-center justify-center rounded-hw bg-route/10 text-route" aria-hidden="true">
          {!selectedFile.videoCodec && selectedFile.audioCodec ? <Note className="size-5" /> : !selectedFile.durationSeconds ? <Frame className="size-5" /> : <Film className="size-5" />}
        </span>
        <div className="min-w-0 flex-1">
          <h2 className="break-words text-[18px] font-medium leading-snug text-ink" style={{ overflowWrap: "anywhere" }}>{basename(selectedFile.path)}</h2>
          <p className="mt-1 text-[12px] text-ink-dim">{[selectedFile.extension.toUpperCase(), selectedFile.width && selectedFile.height ? `${selectedFile.width} × ${selectedFile.height}` : null, selectedFile.durationSeconds ? formatEta(selectedFile.durationSeconds) : null, selectedFile.bytes ? formatBytes(selectedFile.bytes) : null].filter(Boolean).join(" · ")}</p>
          <p data-selectable className="mt-1 truncate font-mono text-[11px] text-ink-faint" title={selectedFile.path}>{selectedFile.path}</p>
        </div>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-3">
        {convert.files.length > 1 ? <div className="min-w-0 flex-1 sm:max-w-sm"><Field label="Selected file" hint={`Settings apply to all ${convert.files.length} staged files unless a file gets its own.`}><SelectInput ariaLabel="Selected file" value={selectedFile.path} onChange={convert.selectFile} options={convert.files.map((file) => ({ value: file.path, label: basename(file.path) }))} /></Field></div> : <p className="text-[12px] text-ink-dim">Choose an output and adjust its settings below.</p>}
        <div className="flex flex-wrap items-center gap-2">
          <Button tone="ghost" disabled={convert.scanning || convert.starting || !hasEngine()} onClick={() => void browseFiles()}>{convert.scanning ? "Reading…" : "Add files"}</Button>
          <button type="button" aria-expanded={managingFiles} aria-controls="convert-file-manager" disabled={convert.scanning || convert.starting} onClick={() => setManagingFiles((previous) => !previous)} className="focus-ring rounded-hw px-2 py-1.5 text-[12px] text-ink-dim hover:text-ink disabled:opacity-50">{managingFiles ? "Done" : "Change files"}</button>
        </div>
      </div>
      {convert.files.length > 1 && <ConversionToggle checked={convert.customized} onChange={setIndividualSettings} label="Use separate settings for this file" description="Everything below then applies only to the selected file; the other files keep the shared settings." />}
    </Section>}
    {(!convert.capabilities || !convert.capabilities.ffprobe) && hasEngine() && <div className="flex flex-wrap items-center justify-between gap-3 border border-edge p-3">
        <p className="text-[12px] text-ink-dim">FFmpeg and FFprobe are required. Formats follow the capabilities of your installed tools.</p>
        <Button tone="ghost" disabled={installing} onClick={() => void install()}>{installing ? "Installing…" : "Install / repair tools"}</Button>
    </div>}
    {installError && <ErrorStrip message={installError} onDismiss={() => setInstallError(null)} />}
    {convert.error && <ErrorStrip message={convert.error} onDismiss={convert.clearError} />}
    {dropNotice && <ErrorStrip message={dropNotice} onDismiss={() => setDropNotice(null)} />}
    {(!selectedFile || managingFiles) && <Section id={selectedFile ? "convert-file-manager" : "section-files"} title={selectedFile ? "Manage files" : "Files"} description={selectedFile ? "Add paths or remove inputs. Your conversion settings stay as they are." : "Convert local video, audio, images, and animations. Drop files or folders, choose files, or paste one path per line. Originals are kept by default."}
      aside={selectedFile ? <ClearButton disabled={convert.scanning || convert.starting} onClick={() => { convert.clearFiles(); convert.clearError(); setManagingFiles(false); }} /> : undefined}>
      <div {...dropProps} className="relative rounded-hw-lg">
      {dragging && <div aria-hidden="true" className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center rounded-hw-lg border-2 border-dashed border-route bg-panel/85"><span className="legend text-[12px] text-route">Drop to add to the converter</span></div>}
      {!selectedFile && <div className="flex flex-col items-center gap-3 rounded-hw-lg border border-edge-strong bg-recess px-6 py-7 text-center">
        <Transfer className="size-6 text-ink-faint" /><p className="text-[13px] text-ink-dim">{convert.scanning ? "Reading media…" : "Video, audio, images, or animations"}</p>
        <Button tone="ghost" disabled={convert.scanning || convert.starting || !hasEngine()} onClick={() => void browseFiles()}>Choose files</Button>
        <p className="text-[11px] text-ink-faint">Folders: up to 500 files, two directory levels. Unsupported files are excluded.</p>
      </div>}
      {selectedFile && <div className="recess rounded-hw-lg">{convert.files.map((file) => <div key={file.path} className="flex min-w-0 items-center justify-between gap-3 border-b border-edge px-3 py-2 last:border-b-0"><span className="truncate text-[12px] text-ink" title={file.path}>{basename(file.path)}</span><button type="button" className="focus-ring p-1 text-ink-dim" disabled={convert.starting} aria-label={`Remove input ${basename(file.path)}`} onClick={() => convert.removeFile(file.path)}><Close className="size-4" /></button></div>)}</div>}
      <div className="flex items-start gap-2"><textarea aria-label="Paths to add" autoComplete="off" spellCheck={false} value={pathDraft} onChange={(event) => setPathDraft(event.target.value)} rows={1} placeholder="Paste file or folder paths, one per line" className="conversion-paths-input recess focus-ring min-w-0 flex-1 rounded-hw px-3 py-2 font-mono text-[12px] text-ink" /><Button tone="ghost" onClick={() => void addPaths()} disabled={!pathDraft.trim() || convert.scanning || convert.starting}>Add</Button></div>
      </div>
    </Section>}
    {convert.files.length > 0 && <>
      <Section title="Output" description="Automatic mode preserves compatible streams. Choose Encode when changing quality or bitrate.">
        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="Output format" info={<ConversionHelp topic="format" />}><SelectInput ariaLabel="Output format" value={convert.settings.format} onChange={(format) => convert.setSettings((previous) => adaptSettingsToFormat(previous, format as ConversionFormat))}
            options={CONVERSION_FORMATS.map((format) => ({ value: format.value, label: `${format.kind} · ${format.label}`, group: format.kind, badge: `.${format.value === "alac" ? "m4a" : format.value}`, disabled: Boolean(available && !available.includes(format.value)) }))} /></Field>
          <Field label="Preset"><SelectInput ariaLabel="Preset" value={presetValue} onChange={applyPreset}
            options={[{ value: "", label: "Choose a starting point…" }, ...(presetValue === "custom" ? [{ value: "custom", label: "Custom", disabled: true }] : []), { value: "default", label: "Automatic · preserve compatible streams", group: "Quick" }, { value: "small", label: "Smaller file · encode", group: "Quick" }, { value: "quality", label: "High quality · encode", group: "Quick" }, { value: "gif", label: "Balanced GIF · first 5 seconds, 640 px, 15 fps", group: "Quick", disabled: !presetSupportsFormat("gif", convert.settings.format) || Boolean(available && !available.includes("gif")) }, { value: "voice", label: "Voice · mono Opus, normalized", group: "Quick", disabled: !presetSupportsFormat("voice", convert.settings.format) || Boolean(available && !available.includes("opus")) }, ...CONVERSION_PRESET_LIBRARY.map((entry) => ({ value: entry.id, label: `${entry.group} · ${entry.label}`, group: entry.group, disabled: !presetSupportsFormat(entry.id, convert.settings.format) || Boolean(available && !available.includes(entry.settings.format)) })), ...Object.keys(presets).map((name) => ({ value: `saved:${name}`, label: `Saved · ${name}`, group: "Saved", disabled: !presetSupportsFormat(`saved:${name}`, convert.settings.format, presets) }))]} /></Field>
        </div>
        {available && !available.includes(convert.settings.format) && <p role="alert" className="text-[12px] text-danger">The selected format is unavailable. Choose a supported format.</p>}
        <SettingsFields convert={convert} />
        <div className="flex flex-wrap items-center gap-2"><TextInput value={presetName} onChange={setPresetName} ariaLabel="Preset name" placeholder="Name these settings" className="max-w-xs" /><Button tone="ghost" disabled={!presetName.trim() || !ready} onClick={savePreset}>Save preset</Button></div>
        <Field label="Filename pattern" hint={nameError ?? <>Preview: <span data-selectable className="font-mono">{previewOutputName(convert.settings.nameTemplate, basename(selectedFile?.path ?? "example.mp4"), convert.settings.format)}</span>. Existing files are never replaced; a number is added instead.</>}>
          <div className="flex flex-wrap items-start gap-2"><TextInput value={convert.settings.nameTemplate} onChange={(value) => convert.setSettings((previous) => ({ ...previous, nameTemplate: value }))} ariaLabel="Filename pattern" placeholder="{name}" mono className="min-w-[12rem] flex-1" />
            {NAME_TOKENS.map((token) => <Button key={token} tone="ghost" onClick={() => convert.setSettings((previous) => ({ ...previous, nameTemplate: `${previous.nameTemplate}{${token}}` }))}>{`{${token}}`}</Button>)}</div>
        </Field>
        <Field label="Destination folder" hint="Leave empty to save in the Converted subfolder of your current working folder. Existing files receive a unique name."><div className="flex items-start gap-2"><TextInput value={convert.directory} onChange={convert.setDirectory} ariaLabel="Destination folder" placeholder={convert.defaultDirectory ?? "Working folder/Converted"} mono className="flex-1" /><Button tone="ghost" onClick={() => void convert.browseDirectory()}>Browse</Button></div></Field>
        <details className="border-t border-edge pt-3"><summary className="focus-ring cursor-pointer text-[12px] text-ink-dim">Original files</summary><div className="mt-3"><ConversionToggle checked={convert.replaceSource} onChange={convert.setReplaceSource} label="Delete originals only after verified conversion" description="This removes source files after successful conversion. Keep this off to preserve originals." /></div></details>
      </Section>
      {selectedFile && <Section title="Trim and crop" description="Drag the handles to keep part of the file, and the box to keep part of the picture. Nothing changes until you convert.">
        <TrimCropEditor key={`${selectedFile.path}:${kind === "Audio"}`} file={kind === "Audio" ? { ...selectedFile, videoCodec: null } : selectedFile} settings={convert.settings} onChange={(patch) => convert.setSettings((previous) => ({ ...previous, ...patch }))} />
      </Section>}
      {selectedFile && <Section title="Preview" description="Render a few seconds with these settings to check the look and the size.">
        <ConvertPreview key={selectedFile.path} file={selectedFile} settings={convert.settings} outputSeconds={convert.plans.find((row) => row.path === selectedFile.path)?.plan?.durationSeconds ?? null} />
      </Section>}
      <Section title="Review" description={convert.planning ? "Checking settings against each source…" : "Review the operations and any information that will be removed before starting."}>
        <div className="recess rounded-hw-lg">{convert.files.map((file) => {
          const row = convert.plans.find((plan) => plan.path === file.path);
          const selected = convert.selection?.path === file.path;
          return <div key={file.path} data-conversion-path={file.path} data-conversion-selected={selected || undefined} tabIndex={-1} role="group" aria-label={`Conversion input ${basename(file.path)}`} aria-current={selected ? "true" : undefined} className={`focus-ring flex items-start gap-3 border-b border-edge px-4 py-3 last:border-b-0 ${selected ? "bg-route/10" : ""}`}>
            <div className="min-w-0 flex-1"><p className="truncate text-[13px] text-ink" title={file.path}>{basename(file.path)} → .{row?.plan?.outputExtension ?? convert.settings.format}{convert.customizedPaths.includes(file.path) && <Legend className="ml-2">own settings</Legend>}</p>
              <p className="mt-1 text-[11px] text-ink-dim">{[file.width && file.height ? `${file.width} × ${file.height}` : null, file.videoCodec, file.audioCodec, file.bytes ? formatBytes(file.bytes) : null, file.durationSeconds ? formatEta(file.durationSeconds) : null].filter(Boolean).join(" · ")}</p>
              <p className={`mt-1 text-[12px] ${row?.error ? "text-danger" : "text-clear"}`}>{row?.error ?? row?.plan?.description ?? "Checking…"}</p>
              {row?.plan?.warnings.map((warning) => <p key={warning} className="mt-1 text-[11px] text-caution">{warning}</p>)}
            </div><button className="focus-ring p-1 text-ink-dim" disabled={convert.starting} aria-label={`Remove ${basename(file.path)}`} onClick={() => convert.removeFile(file.path)}><Close className="size-4" /></button>
          </div>;
        })}</div>
        <div className="flex flex-wrap items-center justify-between gap-3"><p aria-live="polite" className="text-[12px] text-ink-dim">{compatible} ready · {convert.files.length - compatible} excluded{convert.files.length > compatible ? "; excluded files stay in the list" : ""}</p><Button disabled={!ready || convert.starting || convert.scanning} onClick={() => void run()}>{convert.starting ? "Queuing…" : `Convert ${compatible} compatible ${compatible === 1 ? "file" : "files"}`}</Button></div>
      </Section>
    </>}
    <ConvertJobs convert={convert} />
    <Section title="Auto-convert" description="Convert new files that appear in a folder, such as Downloads, using the settings currently shown. Files already there are never touched.">
      <ConvertWatchRules current={convert.settings} defaultFolder={convert.defaultDirectory} />
    </Section>
    {dropIssue && <UnsupportedFilesModal names={dropIssue.names} addedCount={dropIssue.added} onClose={() => setDropIssue(null)} />}
  </div>;
}
