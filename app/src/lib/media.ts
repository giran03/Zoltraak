/**
 * What the window knows about a media file without asking the engine.
 *
 * Two kinds of thing live here. The path helpers are the ordinary sort. The
 * codec table is not: it is a *preview* of a decision the engine makes for
 * itself, kept here so a row can say "this one needs a real encode" before
 * anything is started. The engine probes the actual file and plans it again
 * on its own, so drift between the two costs a wrong caption, never a wrong
 * run — and there is no import that could share the table, since one side of
 * it is Rust.
 *
 * Deliberately free of React and of the engine bridge, so it can be unit
 * tested on its own.
 */
import type { Container, MediaInfo, RemuxPlan } from "./engine";

/** The containers the engine will write, in the order the picker shows them. */
export const CONTAINERS: { value: Container; label: string; hint: string }[] = [
  { value: "mp4", label: "MP4", hint: "Plays everywhere — phones, editors, Discord, the web." },
  { value: "mkv", label: "MKV", hint: "Holds any codec as it is, so it is always a straight copy." },
  { value: "mov", label: "MOV", hint: "What QuickTime and Final Cut expect." },
  { value: "webm", label: "WebM", hint: "For the open web. VP9 and Opus only." },
];

/** Mirrors `Container::accepts_video` / `accepts_audio` in the engine. */
const ACCEPTS: Record<Container, { video: string[]; audio: string[] }> = {
  mp4: { video: ["h264", "hevc", "av1", "mpeg4"], audio: ["aac", "mp3", "ac3", "eac3", "alac"] },
  mkv: { video: ["*"], audio: ["*"] },
  mov: { video: ["h264", "hevc", "prores", "mpeg4", "mjpeg"], audio: ["aac", "mp3", "alac"] },
  webm: { video: ["vp8", "vp9", "av1"], audio: ["opus", "vorbis"] },
};

/** The plan the engine will arrive at for this file, for the row's caption. */
export function planFor(info: MediaInfo, container: Container): RemuxPlan {
  const table = ACCEPTS[container];
  const accepts = (list: string[], codec: string) =>
    list[0] === "*" ||
    list.includes(codec) ||
    // QuickTime's uncompressed audio is a family of codec names, not one.
    (container === "mov" && codec.startsWith("pcm"));

  const decide = (codec: string | null, list: string[]): RemuxPlan["video"] => {
    if (codec) return accepts(list, codec) ? "copy" : "encode";
    // An unprobed file has unknown streams rather than none, and the engine
    // plans those as a transcode.
    return info.unprobed ? "encode" : "absent";
  };
  return {
    video: decide(info.videoCodec, table.video),
    audio: decide(info.audioCodec, table.audio),
  };
}

/** Whether the run copies the streams rather than re-encoding either of them. */
export function isLossless(plan: RemuxPlan): boolean {
  return plan.video !== "encode" && plan.audio !== "encode";
}

/** One line on what a conversion would do to this file. */
export function describePlan(plan: RemuxPlan, container: Container): string {
  const target = `.${container}`;
  if (plan.video === "encode" && plan.audio === "encode") return `re-encodes both streams for ${target}`;
  if (plan.video === "encode") return `re-encodes the picture for ${target}`;
  if (plan.audio === "encode") return `re-encodes the sound for ${target}`;
  return `copies the streams into ${target}`;
}

/** The file's own name, off the end of its path. */
export function basename(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/** The folder a path sits in. */
export function dirname(path: string): string {
  const parts = path.split(/[\\/]/);
  parts.pop();
  return parts.join(path.includes("\\") ? "\\" : "/");
}

/** The lowercased extension of a path, or `""` when it has none. */
export function extensionOf(path: string): string {
  const name = basename(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

/** Whether repackaging this extension into `container` would be a no-op. */
export function extensionIsContainer(extension: string, container: Container): boolean {
  const ext = extension.toLowerCase();
  if (container === "mp4") return ext === "mp4" || ext === "m4v";
  return ext === container;
}

export function alreadyIs(info: MediaInfo, container: Container): boolean {
  return extensionIsContainer(info.extension, container);
}

/** Mirrors `MEDIA_EXTENSIONS` in the engine. */
const MEDIA_EXTENSIONS = new Set([
  "mp4", "m4v", "mkv", "webm", "mov", "avi", "flv", "ts", "m2ts", "mts", "mpg", "mpeg", "wmv",
  "3gp", "ogv", "vob", "asf", "rm", "rmvb", "divx", "f4v", "m4a", "mp3", "aac", "flac", "opus",
  "ogg", "wav", "wma", "alac", "aiff", "jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff", "apng", "m4b",
]);

/** The subset of the above that carries a picture. */
const VIDEO_EXTENSIONS = new Set([
  "mp4", "m4v", "mkv", "webm", "mov", "avi", "flv", "ts", "m2ts", "mts", "mpg", "mpeg", "wmv",
  "3gp", "ogv", "vob", "asf", "rm", "rmvb", "divx", "f4v",
]);

export function isMediaPath(path: string): boolean {
  return MEDIA_EXTENSIONS.has(extensionOf(path));
}

export function isVideoPath(path: string): boolean {
  return VIDEO_EXTENSIONS.has(extensionOf(path));
}
