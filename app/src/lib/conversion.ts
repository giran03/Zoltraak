import type { ConversionFormat, ConversionSettings, MediaInfo, RemuxJob } from "./engine";

export function conversionDirectory(workingDirectory?: string | null): string | null {
  if (!workingDirectory?.trim()) return null;
  const folder = workingDirectory.trim();
  const separator = folder.includes("\\") ? "\\" : "/";
  return `${folder.replace(/[\\/]+$/, "")}${separator}Converted`;
}

export const CONVERSION_FORMATS: { value: ConversionFormat; label: string; kind: "Video" | "Image" | "Animation" | "Audio" }[] = [
  { value: "mp4", label: "MP4 · H.264 / AAC", kind: "Video" },
  { value: "mkv", label: "MKV · Matroska", kind: "Video" },
  { value: "mov", label: "MOV · QuickTime", kind: "Video" },
  { value: "webm", label: "WebM · VP9 / Opus", kind: "Video" },
  { value: "gif", label: "GIF", kind: "Animation" },
  { value: "webp", label: "WebP · still or animated", kind: "Animation" },
  { value: "png", label: "PNG", kind: "Image" },
  { value: "jpg", label: "JPEG", kind: "Image" },
  { value: "bmp", label: "BMP", kind: "Image" },
  { value: "tiff", label: "TIFF", kind: "Image" },
  { value: "mp3", label: "MP3", kind: "Audio" },
  { value: "m4a", label: "M4A · AAC", kind: "Audio" },
  { value: "alac", label: "M4A · Apple Lossless", kind: "Audio" },
  { value: "flac", label: "FLAC", kind: "Audio" },
  { value: "wav", label: "WAV · 16-bit PCM", kind: "Audio" },
  { value: "aiff", label: "AIFF · 16-bit PCM", kind: "Audio" },
  { value: "opus", label: "Opus", kind: "Audio" },
  { value: "ogg", label: "Ogg Vorbis", kind: "Audio" },
];

export function defaultConversionSettings(format: ConversionFormat = "mp4"): ConversionSettings {
  return { format, mode: "auto", quality: 80, width: null, height: null, fps: null, startSeconds: 0,
    endSeconds: null, durationSeconds: null, mute: false, audioBitrate: 192, sampleRate: null,
    channels: null, audioTrack: 0, normalize: false, fadeIn: 0, fadeOut: 0, stripMetadata: false,
    normalizeTwoPass: false, loudnessLufs: -16, truePeakDb: -1.5,
    background: "#ffffff", loopCount: 0, dither: "sierra2_4a", lossless: false,
    targetSizeMb: null, nameTemplate: DEFAULT_NAME_TEMPLATE,
    crop: null, rotate: 0, flipHorizontal: false, flipVertical: false };
}

export const DEFAULT_NAME_TEMPLATE = "{name}";
export const NAME_TOKENS = ["name", "format", "date", "time"] as const;
const RESERVED_NAMES = new Set(["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "LPT1", "LPT2"]);

/** Mirrors the engine's pattern rules; returns the problem, or null when the pattern is usable. */
export function nameTemplateError(template: string): string | null {
  if (!template.trim() || [...template].length > 120) return "Use 1 to 120 characters";
  for (const [piece] of template.matchAll(/\{[^{}]*\}|[{}]/g)) {
    if (piece === "{") return "Unmatched {";
    if (piece === "}") return "Unmatched }";
    if (!(NAME_TOKENS as readonly string[]).includes(piece.slice(1, -1))) return "Use only {name}, {format}, {date} and {time}";
  }
  return null;
}

/** What the engine will name the file, before it adds " (2)" to avoid an existing file. */
export function previewOutputName(template: string, sourceName: string, format: ConversionFormat, now = new Date()): string {
  const stem = sourceName.replace(/\.[^.\\/]+$/, "");
  const two = (n: number) => String(n).padStart(2, "0");
  const expanded = template
    .replaceAll("{name}", stem).replaceAll("{format}", format)
    .replaceAll("{date}", `${now.getFullYear()}-${two(now.getMonth() + 1)}-${two(now.getDate())}`)
    .replaceAll("{time}", `${two(now.getHours())}-${two(now.getMinutes())}-${two(now.getSeconds())}`);
  const cleaned = expanded.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim().replace(/\.+$/, "").trim() || stem;
  return `${RESERVED_NAMES.has(cleaned.toUpperCase()) ? `_${cleaned}` : cleaned}.${format}`;
}

/** Formats that can be steered toward a size budget. */
export function supportsTargetSize(format: ConversionFormat): boolean {
  return ["mp4", "mkv", "mov", "webm", "mp3", "m4a", "opus", "ogg"].includes(format);
}

export const SAMPLE_RATES = [8000, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 88200, 96000, 192000] as const;
export const OPUS_SAMPLE_RATES = [8000, 12000, 16000, 24000, 48000] as const;

export function formatKind(format: ConversionFormat): "Video" | "Image" | "Animation" | "Audio" {
  return CONVERSION_FORMATS.find((entry) => entry.value === format)?.kind ?? "Video";
}

export function supportsStreamCopy(format: ConversionFormat): boolean {
  const kind = formatKind(format);
  return kind === "Video" || kind === "Audio";
}

export function supportsQuality(format: ConversionFormat): boolean {
  return formatKind(format) !== "Audio" && !["gif", "bmp", "tiff"].includes(format);
}

export function supportsBackground(format: ConversionFormat): boolean {
  return ["jpg", "bmp", "mp4", "mov", "mkv", "webm"].includes(format);
}

export function supportsSampleRate(format: ConversionFormat, rate: number): boolean {
  return format !== "opus" || (OPUS_SAMPLE_RATES as readonly number[]).includes(rate);
}

/** Whether a preset option is compatible with the currently selected output format. */
export function presetSupportsFormat(
  presetId: string,
  format: ConversionFormat,
  savedPresets?: Record<string, ConversionSettings>,
): boolean {
  if (!presetId || ["custom", "default", "small", "quality"].includes(presetId)) return true;
  if (presetId === "gif") return format === "gif";
  if (presetId === "voice") return format === "opus";
  if (presetId.startsWith("saved:")) return savedPresets?.[presetId.slice(6)]?.format === format;
  const library = CONVERSION_PRESET_LIBRARY.find((entry) => entry.id === presetId);
  return library ? library.settings.format === format : true;
}

/** Keep customized settings when changing format while resetting values the new format rejects. */
export function adaptSettingsToFormat(previous: ConversionSettings, format: ConversionFormat): ConversionSettings {
  const next: ConversionSettings = { ...previous, format };
  if (!supportsStreamCopy(format) && next.mode === "copy") next.mode = "auto";
  if (!supportsTargetSize(format) && next.targetSizeMb !== null) next.targetSizeMb = null;
  if (next.sampleRate !== null && !supportsSampleRate(format, next.sampleRate)) next.sampleRate = null;
  if (formatKind(format) === "Audio") {
    if (next.mute) next.mute = false;
    if (next.crop !== null) next.crop = null;
    if (next.rotate !== 0) next.rotate = 0;
    if (next.flipHorizontal) next.flipHorizontal = false;
    if (next.flipVertical) next.flipVertical = false;
  }
  return next;
}

/** Which conversion fields are supported by the current output settings. */
export function outputFieldSupport(settings: ConversionSettings) {
  const kind = formatKind(settings.format);
  const picture = kind !== "Audio";
  const sound = kind === "Audio" || kind === "Video";
  const copying = settings.mode === "copy";
  const encodingPicture = picture && !copying;
  const encodingAudio = sound && !settings.mute && !copying;
  const losslessAudio = ["flac", "alac", "wav", "aiff"].includes(settings.format);
  return {
    streamCopy: supportsStreamCopy(settings.format),
    quality: encodingPicture && supportsQuality(settings.format) && !(kind === "Video" && settings.targetSizeMb !== null) && !(settings.format === "webp" && settings.lossless),
    targetSize: !copying && supportsTargetSize(settings.format),
    endTime: kind !== "Image",
    dimensions: encodingPicture,
    fps: (kind === "Video" || kind === "Animation") && !copying,
    duration: kind !== "Image" && settings.endSeconds === null,
    audioBitrate: encodingAudio && !losslessAudio && !(kind === "Audio" && settings.targetSizeMb !== null),
    audioTrack: sound && !settings.mute,
    background: encodingPicture && supportsBackground(settings.format),
    loopCount: kind === "Animation" && !copying,
    dither: settings.format === "gif" && !copying,
    losslessWebp: settings.format === "webp" && !copying,
    audioProcessing: encodingAudio,
    loudnessTargets: encodingAudio && settings.normalize,
    mute: kind === "Video",
    cropAndOrientation: encodingPicture,
  };
}

export type ConversionPresetEntry = {
  id: string; group: "Share" | "Video" | "Audio" | "Image"; label: string;
  /** Settings layered over the defaults for `format`. */
  settings: Partial<ConversionSettings> & { format: ConversionFormat };
};

/** One-click targets. Each re-encodes, because every one asks for a specific result. */
export const CONVERSION_PRESET_LIBRARY: ConversionPresetEntry[] = [
  { id: "discord-10", group: "Share", label: "Discord · under 10 MB", settings: { format: "mp4", mode: "encode", targetSizeMb: 10, width: 1280, height: 720, audioBitrate: 96 } },
  { id: "messaging-16", group: "Share", label: "WhatsApp / Telegram · under 16 MB", settings: { format: "mp4", mode: "encode", targetSizeMb: 16, width: 1280, height: 720, audioBitrate: 96 } },
  { id: "email-25", group: "Share", label: "Email · under 25 MB", settings: { format: "mp4", mode: "encode", targetSizeMb: 25, width: 1280, height: 720, audioBitrate: 128 } },
  { id: "social-gif", group: "Share", label: "Social GIF · 480 px, 12 fps, 6 s", settings: { format: "gif", mode: "encode", width: 480, fps: 12, durationSeconds: 6 } },
  { id: "web-mp4", group: "Video", label: "Web-safe MP4 · up to 1080p", settings: { format: "mp4", mode: "encode", quality: 80, width: 1920, height: 1080, audioBitrate: 160 } },
  { id: "web-webm", group: "Video", label: "Compact WebM · up to 720p", settings: { format: "webm", mode: "encode", quality: 60, width: 1280, height: 720, audioBitrate: 96 } },
  { id: "podcast-mp3", group: "Audio", label: "Podcast MP3 · mono, 96 kbps, normalized", settings: { format: "mp3", mode: "encode", channels: 1, audioBitrate: 96, sampleRate: 44100, normalize: true } },
  { id: "music-mp3", group: "Audio", label: "Music MP3 · 320 kbps", settings: { format: "mp3", mode: "encode", audioBitrate: 320 } },
  { id: "archive-flac", group: "Audio", label: "Lossless archive · FLAC", settings: { format: "flac", mode: "encode" } },
  { id: "web-jpeg", group: "Image", label: "Web JPEG · 1600 px, no metadata", settings: { format: "jpg", mode: "encode", quality: 85, width: 1600, height: 1600, stripMetadata: true } },
  { id: "web-png", group: "Image", label: "Web PNG · 1600 px, no metadata", settings: { format: "png", mode: "encode", width: 1600, height: 1600, stripMetadata: true } },
];

/** Library entry by id, layered over defaults; null when the id is not a library preset. */
export function libraryPreset(id: string): ConversionSettings | null {
  const entry = CONVERSION_PRESET_LIBRARY.find((candidate) => candidate.id === id);
  return entry ? { ...defaultConversionSettings(entry.settings.format), ...entry.settings } : null;
}

export function conversionPreset(name: string, format: ConversionFormat): ConversionSettings {
  const library = libraryPreset(name);
  if (library) return library;
  const settings = defaultConversionSettings(format);
  if (name === "small") return { ...settings, mode: "encode", quality: 55, audioBitrate: 96, width: 854 };
  if (name === "quality") return { ...settings, mode: "encode", quality: 95, audioBitrate: 256 };
  if (name === "gif") return { ...settings, format: "gif", mode: "encode", width: 640, fps: 15, durationSeconds: 5 };
  if (name === "voice") return { ...settings, format: "opus", mode: "encode", channels: 1, audioBitrate: 48, sampleRate: 48000, normalize: true };
  return settings;
}

/** Empty fields preserve the source. Invalid drafts stay invalid for validation. */
export function parseTimestamp(value: string): number | null {
  if (!value.trim()) return null;
  const parts = value.trim().split(":");
  if (parts.length > 3 || parts.some((part) => !/^\d+(?:\.\d+)?$/.test(part))) return NaN;
  if (parts.length > 1 && parts.slice(1).some((part) => Number(part) >= 60)) return NaN;
  return parts.reduce((total, part) => total * 60 + Number(part), 0);
}

/** Files that share one resolved recipe, so they can be planned and queued together. */
export function groupBySettings(paths: string[], resolve: (path: string) => ConversionSettings): { settings: ConversionSettings; paths: string[] }[] {
  const groups = new Map<string, { settings: ConversionSettings; paths: string[] }>();
  for (const path of paths) {
    const settings = resolve(path);
    const key = JSON.stringify(settings);
    const group = groups.get(key);
    if (group) group.paths.push(path); else groups.set(key, { settings, paths: [path] });
  }
  return [...groups.values()];
}

function describeMedia(info: MediaInfo | null | undefined): string | null {
  if (!info) return null;
  const parts = [info.videoCodec ?? info.audioCodec, info.width && info.height ? `${info.width}×${info.height}` : null];
  return parts.filter(Boolean).join(" ") || null;
}

export type JobComparison = { inputBytes: number; outputBytes: number; savedPercent: number; from: string | null; to: string | null };

/** What a finished conversion did to the file; null until both sizes are known. */
export function compareJob(job: RemuxJob): JobComparison | null {
  if (job.state !== "finished" || !job.inputBytes || !job.outputBytes) return null;
  return {
    inputBytes: job.inputBytes, outputBytes: job.outputBytes,
    savedPercent: Math.round((1 - job.outputBytes / job.inputBytes) * 100),
    from: describeMedia(job.source), to: describeMedia(job.outputInfo),
  };
}

/** A whole-file size guess from a short sample. It is approximate: bitrate varies along a file. */
export function estimateFullBytes(sampleBytes: number, sampleSeconds: number, outputSeconds: number | null): number | null {
  if (!outputSeconds || sampleSeconds <= 0 || sampleBytes <= 0) return null;
  return Math.round((sampleBytes / sampleSeconds) * outputSeconds);
}

/** Pixel size of the result after crop, rotation, and the maximum width and height. Null when the source size is unknown. */
export function resultingSize(info: Pick<MediaInfo, "width" | "height">, settings: Pick<ConversionSettings, "crop" | "rotate" | "width" | "height">): { width: number; height: number } | null {
  if (!info.width || !info.height) return null;
  const even = (n: number) => Math.max(2, Math.floor(n / 2) * 2);
  let width = even(info.width * (settings.crop?.width ?? 1));
  let height = even(info.height * (settings.crop?.height ?? 1));
  if (settings.rotate === 90 || settings.rotate === 270) [width, height] = [height, width];
  const scale = Math.min(1, settings.width ? settings.width / width : Infinity, settings.height ? settings.height / height : Infinity);
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

export type SelectOption<T extends string = string> = {
  value: T;
  label: string;
  group?: string;
  badge?: string;
  detail?: string;
  disabled?: boolean;
};

export type ParsedSelectOption<T extends string = string> = {
  value: T;
  rawLabel: string;
  primary: string;
  badge?: string;
  detail?: string;
  group?: string;
  disabled?: boolean;
};

export function describeSelectOption<T extends string>(option: SelectOption<T>): ParsedSelectOption<T> {
  let text = option.label.trim();
  if (option.group && text.toLowerCase().startsWith(`${option.group.toLowerCase()} · `)) {
    text = text.slice(option.group.length + 3).trim();
  }
  let detail = option.detail;
  let badge = option.badge;
  if (!detail) {
    if (text.includes(" · ")) {
      const [head = "", ...rest] = text.split(" · ");
      text = head.trim();
      detail = rest.join(" · ").trim();
    } else if (text.includes(" — ")) {
      const [head = "", ...rest] = text.split(" — ");
      text = head.trim();
      detail = rest.join(" — ").trim();
    }
  }
  const paren = /^(.+?)\s+\(([^)]+)\)$/.exec(text);
  if (paren && paren[1] && paren[2]) {
    const inner = paren[2].trim();
    if (/^\.[a-z0-9]+$/i.test(inner) && !badge) {
      text = paren[1].trim();
      badge = `(${inner})`;
    } else if (!detail) {
      text = paren[1].trim();
      detail = inner;
    }
  }
  if (badge && !badge.startsWith("(")) badge = `(${badge})`;
  return {
    value: option.value,
    rawLabel: option.label,
    primary: text,
    badge,
    detail,
    group: option.group,
    disabled: option.disabled,
  };
}

export function groupSelectOptions<T extends string>(
  options: ParsedSelectOption<T>[],
): { group: string | null; items: { option: ParsedSelectOption<T>; index: number }[] }[] {
  const sections: { group: string | null; items: { option: ParsedSelectOption<T>; index: number }[] }[] = [];
  options.forEach((option, index) => {
    const group = option.group ?? null;
    const last = sections[sections.length - 1];
    if (last && last.group === group) {
      last.items.push({ option, index });
    } else {
      sections.push({ group, items: [{ option, index }] });
    }
  });
  return sections;
}
