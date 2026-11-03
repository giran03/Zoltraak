import type { ConversionCrop, ConversionSettings } from "./engine";

/** The smallest side a crop or trim can shrink to, as a fraction of the picture or in seconds. */
export const MIN_CROP_SIDE = 0.05;
export const MIN_TRIM_SECONDS = 0.1;

export const FULL_CROP: ConversionCrop = { x: 0, y: 0, width: 1, height: 1 };
export type CropCorner = "nw" | "ne" | "sw" | "se";
export type QuarterTurn = ConversionSettings["rotate"];
export type PictureOrientation = Pick<ConversionSettings, "rotate" | "flipHorizontal" | "flipVertical">;

const clamp = (value: number, low: number, high: number) => Math.min(high, Math.max(low, value));

export function isFullCrop(crop: ConversionCrop | null): boolean {
  return !crop || (crop.x <= 0 && crop.y <= 0 && crop.width >= 1 && crop.height >= 1);
}

/** Slide a crop without changing its size, keeping it inside the picture. */
export function moveCrop(crop: ConversionCrop, dx: number, dy: number): ConversionCrop {
  return {
    ...crop,
    x: clamp(crop.x + dx, 0, 1 - crop.width),
    y: clamp(crop.y + dy, 0, 1 - crop.height),
  };
}

/** Drag one corner to (px, py); the opposite corner stays where it is. */
export function resizeCrop(crop: ConversionCrop, corner: CropCorner, px: number, py: number): ConversionCrop {
  const anchorX = corner.endsWith("w") ? crop.x + crop.width : crop.x;
  const anchorY = corner.startsWith("n") ? crop.y + crop.height : crop.y;
  const pull = (anchor: number, point: number) => {
    const clamped = clamp(point, 0, 1);
    if (Math.abs(clamped - anchor) >= MIN_CROP_SIDE) return clamped;
    // Too small: hold the minimum on whichever side has room.
    const towards = clamped >= anchor ? 1 : -1;
    const wanted = anchor + towards * MIN_CROP_SIDE;
    return wanted > 1 || wanted < 0 ? anchor - towards * MIN_CROP_SIDE : wanted;
  };
  const x = pull(anchorX, px);
  const y = pull(anchorY, py);
  return { x: Math.min(x, anchorX), y: Math.min(y, anchorY), width: Math.abs(x - anchorX), height: Math.abs(y - anchorY) };
}

/** The largest centered crop with the given width-to-height ratio, measured in source pixels. */
export function aspectCrop(sourceWidth: number, sourceHeight: number, ratio: number): ConversionCrop {
  if (!(sourceWidth > 0 && sourceHeight > 0 && ratio > 0)) return FULL_CROP;
  const sourceRatio = sourceWidth / sourceHeight;
  const width = ratio < sourceRatio ? ratio / sourceRatio : 1;
  const height = ratio < sourceRatio ? 1 : sourceRatio / ratio;
  return { x: (1 - width) / 2, y: (1 - height) / 2, width, height };
}

export type TrimRange = { start: number; end: number };

/** Move one trim handle to `value` seconds without crossing the other or leaving the file. */
export function moveTrimHandle(range: TrimRange, handle: "start" | "end", value: number, duration: number): TrimRange {
  if (handle === "start") return { start: clamp(value, 0, range.end - MIN_TRIM_SECONDS), end: range.end };
  return { start: range.start, end: clamp(value, range.start + MIN_TRIM_SECONDS, duration) };
}

function rotateCropClockwise(crop: ConversionCrop, rotate: QuarterTurn): ConversionCrop {
  if (rotate === 90) return { x: 1 - crop.y - crop.height, y: crop.x, width: crop.height, height: crop.width };
  if (rotate === 180) return { x: 1 - crop.x - crop.width, y: 1 - crop.y - crop.height, width: crop.width, height: crop.height };
  if (rotate === 270) return { x: crop.y, y: 1 - crop.x - crop.width, width: crop.height, height: crop.width };
  return crop;
}

/** Map a source-space crop box into the rotated and flipped stage coordinate system. */
export function cropToStage(crop: ConversionCrop, rotate: QuarterTurn, flipHorizontal: boolean, flipVertical: boolean): ConversionCrop {
  const turned = rotateCropClockwise(crop, rotate);
  return {
    x: flipHorizontal ? 1 - turned.x - turned.width : turned.x,
    y: flipVertical ? 1 - turned.y - turned.height : turned.y,
    width: turned.width,
    height: turned.height,
  };
}

/** Map a stage-space crop box back into unrotated, unflipped source coordinates for FFmpeg. */
export function cropFromStage(stageCrop: ConversionCrop, rotate: QuarterTurn, flipHorizontal: boolean, flipVertical: boolean): ConversionCrop {
  const unflipped: ConversionCrop = {
    x: flipHorizontal ? 1 - stageCrop.x - stageCrop.width : stageCrop.x,
    y: flipVertical ? 1 - stageCrop.y - stageCrop.height : stageCrop.y,
    width: stageCrop.width,
    height: stageCrop.height,
  };
  return rotateCropClockwise(unflipped, ((360 - rotate) % 360) as QuarterTurn);
}

/** Stage dimensions after applying quarter-turn rotation to the source aspect ratio. */
export function stageDimensions(width: number | null | undefined, height: number | null | undefined, rotate: QuarterTurn): { width: number; height: number } {
  const sourceWidth = width && width > 0 ? width : 16;
  const sourceHeight = height && height > 0 ? height : 9;
  return rotate === 90 || rotate === 270 ? { width: sourceHeight, height: sourceWidth } : { width: sourceWidth, height: sourceHeight };
}

/**
 * CSS transform that previews FFmpeg's rotation-then-flip filter order inside
 * an element whose box already has the post-rotation aspect ratio.
 */
export function frameTransform(
  rotate: QuarterTurn,
  flipHorizontal: boolean,
  flipVertical: boolean,
  sourceWidth?: number | null,
  sourceHeight?: number | null,
): string | undefined {
  const w = sourceWidth && sourceWidth > 0 ? sourceWidth : 16;
  const h = sourceHeight && sourceHeight > 0 ? sourceHeight : 9;
  const turned = rotate === 90 || rotate === 270;
  const parts: string[] = [];
  if (flipHorizontal || flipVertical) parts.push(`scale(${flipHorizontal ? -1 : 1}, ${flipVertical ? -1 : 1})`);
  if (rotate !== 0) parts.push(`rotate(${rotate}deg)`);
  if (turned && w !== h) parts.push(`scale(${w / h}, ${h / w})`);
  return parts.length ? parts.join(" ") : undefined;
}

/** Relative orientation transform needed to display a sample rendered with `from` as `to`. */
export function relativeOrientation(from: PictureOrientation, to: PictureOrientation): PictureOrientation {
  const probe: ConversionCrop = { x: 0.1, y: 0.2, width: 0.3, height: 0.4 };
  const rendered = cropToStage(probe, from.rotate, from.flipHorizontal, from.flipVertical);
  const target = cropToStage(probe, to.rotate, to.flipHorizontal, to.flipVertical);
  for (const rotate of [0, 90, 180, 270] as const) {
    for (const flipHorizontal of [false, true]) {
      const candidate = cropToStage(rendered, rotate, flipHorizontal, false);
      if (Math.abs(candidate.x - target.x) < 1e-6 && Math.abs(candidate.y - target.y) < 1e-6) {
        return { rotate, flipHorizontal, flipVertical: false };
      }
    }
  }
  return { rotate: 0, flipHorizontal: false, flipVertical: false };
}
