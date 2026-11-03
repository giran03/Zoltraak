import type { VideoFormat } from "./engine";
export type VideoPreset = "best" | "balanced" | "compatible";
export function choosePreset(formats: VideoFormat[], preset: VideoPreset): VideoFormat | undefined {
  // Quality selection applies to one clip. A pasted carousel needs a clip choice.
  if (new Set(formats.map(f => f.id.match(/^clip-(\d+)-/)?.[1]).filter(Boolean)).size > 1) return undefined;
  const audioRank = (f: VideoFormat) => f.requiresMerge || f.hasAudio === true ? 2 : f.hasAudio == null ? 1 : 0;
  let candidates = formats;
  if (preset === "balanced") candidates = formats.filter(f => f.height != null && f.height <= 720);
  if (preset === "compatible") candidates = formats.filter(f => f.container === "mp4" && /^(avc1|h264)/i.test(f.videoCodec ?? "")
    && audioRank(f) > 0 && /^(mp4a|aac)/i.test(f.audioCodec ?? ""));
  return [...candidates].sort((a,b) => audioRank(b) - audioRank(a) || (b.height ?? 0) - (a.height ?? 0) || (b.bitrate ?? 0) - (a.bitrate ?? 0) || (b.filesize ?? 0) - (a.filesize ?? 0))[0];
}
