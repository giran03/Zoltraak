import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent as ReactPointerEvent } from "react";
import { Button } from "./Controls";
import { Legend } from "./Panel";
import { engine, hasEngine, type ConversionSettings, type MediaInfo } from "../lib/engine";
import { FULL_CROP, MIN_TRIM_SECONDS, aspectCrop, cropFromStage, cropToStage, frameTransform, isFullCrop, moveCrop, moveTrimHandle, resizeCrop, stageDimensions, type CropCorner } from "../lib/cropMath";
import { outputFieldSupport, resultingSize } from "../lib/conversion";

const WAVEFORM_BARS = 240;
const FRAME_WIDTH = 960;
const FRAME_DEBOUNCE_MS = 180;
const KEY_STEP_SECONDS = 0.1;
const KEY_BIG_STEP_SECONDS = 1;
const ASPECTS: { label: string; ratio: number }[] = [
  { label: "16:9", ratio: 16 / 9 }, { label: "4:3", ratio: 4 / 3 }, { label: "1:1", ratio: 1 }, { label: "9:16", ratio: 9 / 16 },
];

const round = (value: number, places: number) => Number(value.toFixed(places));
const clock = (seconds: number) => {
  const whole = Math.floor(seconds);
  const tenths = Math.floor((seconds - whole) * 10);
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}.${tenths}`;
};

type Patch = Partial<ConversionSettings>;

/** Choose a time range and a picture area for one file, on the file itself. */
export function TrimCropEditor({ file, settings, onChange }: { file: MediaInfo; settings: ConversionSettings; onChange: (patch: Patch) => void }) {
  const hasPicture = file.videoCodec !== null;
  const duration = file.durationSeconds ?? 0;
  const timed = duration > 0;
  const [scrub, setScrub] = useState(settings.startSeconds);
  const [frame, setFrame] = useState<string | null>(null);
  const [wave, setWave] = useState<number[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const track = useRef<HTMLDivElement>(null);
  const stage = useRef<HTMLDivElement>(null);
  const range = { start: settings.startSeconds, end: settings.endSeconds ?? duration };
  const crop = settings.crop ?? FULL_CROP;
  const support = outputFieldSupport(settings);
  const canEditPicture = support.cropAndOrientation;
  const canEditEnd = support.endTime;
  const stageSize = stageDimensions(file.width, file.height, settings.rotate);
  const stageCrop = cropToStage(crop, settings.rotate, settings.flipHorizontal, settings.flipVertical);
  const transform = frameTransform(settings.rotate, settings.flipHorizontal, settings.flipVertical, file.width, file.height);

  useEffect(() => {
    if (Number.isFinite(settings.startSeconds)) setScrub(settings.startSeconds);
  }, [settings.startSeconds]);

  useEffect(() => {
    if (!hasPicture || !hasEngine()) return;
    let live = true;
    const timer = window.setTimeout(() => {
      engine.conversionThumbnail(file.path, timed ? scrub : 0, FRAME_WIDTH)
        .then((url) => { if (live) { setFrame(url); setProblem(null); } })
        .catch((e) => { if (live) setProblem(e instanceof Error ? e.message : String(e)); });
    }, FRAME_DEBOUNCE_MS);
    return () => { live = false; window.clearTimeout(timer); };
  }, [file.path, scrub, hasPicture, timed]);

  useEffect(() => {
    if (hasPicture || !timed || !hasEngine()) return;
    let live = true;
    engine.conversionWaveform(file.path, WAVEFORM_BARS)
      .then((peaks) => { if (live) setWave(peaks); })
      .catch((e) => { if (live) setProblem(e instanceof Error ? e.message : String(e)); });
    return () => { live = false; };
  }, [file.path, hasPicture, timed]);

  const commitRange = (next: { start: number; end: number }) => onChange({
    startSeconds: round(next.start, 3),
    endSeconds: next.end >= duration - 0.01 ? null : round(next.end, 3),
  });
  const secondsAt = (clientX: number) => {
    const box = track.current?.getBoundingClientRect();
    return box && box.width > 0 ? Math.min(1, Math.max(0, (clientX - box.left) / box.width)) * duration : 0;
  };
  const dragHandle = (handle: "start" | "end") => (event: ReactPointerEvent<HTMLElement>) => {
    if (handle === "end" && !canEditEnd) return;
    const target = event.currentTarget;
    target.focus({ preventScroll: true });
    target.setPointerCapture(event.pointerId);
    const move = (moved: PointerEvent) => {
      const next = moveTrimHandle(range, handle, secondsAt(moved.clientX), duration);
      commitRange(next);
      setScrub(handle === "start" ? next.start : next.end);
    };
    const stop = () => { target.removeEventListener("pointermove", move); target.removeEventListener("pointerup", stop); };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", stop);
  };
  const keyHandle = (handle: "start" | "end") => (event: KeyboardEvent<HTMLElement>) => {
    if (handle === "end" && !canEditEnd) return;
    const direction = event.key === "ArrowRight" || event.key === "ArrowUp" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowDown" ? -1 : 0;
    if (!direction) return;
    event.preventDefault();
    const step = event.shiftKey ? KEY_BIG_STEP_SECONDS : KEY_STEP_SECONDS;
    const next = moveTrimHandle(range, handle, range[handle] + direction * step, duration);
    commitRange(next);
    setScrub(handle === "start" ? next.start : next.end);
  };

  const fractionAt = (point: { clientX: number; clientY: number }) => {
    const box = stage.current?.getBoundingClientRect();
    return box && box.width > 0 && box.height > 0 ? { x: (point.clientX - box.left) / box.width, y: (point.clientY - box.top) / box.height } : { x: 0, y: 0 };
  };
  const setCrop = (next: typeof crop) => onChange({
    crop: isFullCrop(next) ? null : { x: round(next.x, 4), y: round(next.y, 4), width: round(next.width, 4), height: round(next.height, 4) },
  });
  const dragCrop = (kind: "move" | CropCorner) => (event: ReactPointerEvent<HTMLElement>) => {
    if (!canEditPicture) return;
    event.preventDefault();
    event.stopPropagation();
    const target = event.currentTarget;
    target.setPointerCapture(event.pointerId);
    const origin = fractionAt(event);
    const start = stageCrop;
    const move = (moved: PointerEvent) => {
      const point = fractionAt(moved);
      const nextStage = kind === "move" ? moveCrop(start, point.x - origin.x, point.y - origin.y) : resizeCrop(start, kind, point.x, point.y);
      setCrop(cropFromStage(nextStage, settings.rotate, settings.flipHorizontal, settings.flipVertical));
    };
    const stop = () => { target.removeEventListener("pointermove", move); target.removeEventListener("pointerup", stop); };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", stop);
  };

  const result = hasPicture ? resultingSize(file, settings) : null;
  const percent = (seconds: number) => `${(seconds / duration) * 100}%`;
  const edited = settings.crop !== null || settings.rotate !== 0 || settings.flipHorizontal || settings.flipVertical;

  return <div className="flex flex-col gap-4" data-testid="trim-crop-editor">
    {hasPicture && <div className="flex flex-col gap-2">
      <div ref={stage} className="recess relative mx-auto w-full max-w-xl select-none overflow-hidden rounded-hw-lg" style={{ aspectRatio: `${stageSize.width} / ${stageSize.height}`, backgroundColor: support.background && /^#[0-9a-fA-F]{6}$/.test(settings.background) ? settings.background : undefined }}>
        {frame && <img src={frame} alt={`Frame of ${file.path.split(/[\\/]/).pop()} at ${clock(timed ? scrub : 0)}`} draggable={false} className="absolute inset-0 size-full object-fill" style={transform ? { transform } : undefined} />}
        <div role="group" aria-label="Crop area" aria-disabled={!canEditPicture} className={`absolute touch-none border border-route ${canEditPicture ? "cursor-move" : "cursor-not-allowed opacity-55"}`} onPointerDown={dragCrop("move")}
          style={{ left: `${stageCrop.x * 100}%`, top: `${stageCrop.y * 100}%`, width: `${stageCrop.width * 100}%`, height: `${stageCrop.height * 100}%`, boxShadow: "0 0 0 9999px rgb(0 0 0 / 0.55)" }}>
          {(["nw", "ne", "sw", "se"] as CropCorner[]).map((corner) => <button key={corner} type="button" disabled={!canEditPicture} aria-label={`Resize crop from ${corner === "nw" ? "top left" : corner === "ne" ? "top right" : corner === "sw" ? "bottom left" : "bottom right"}`}
            onPointerDown={dragCrop(corner)} className="focus-ring absolute size-3.5 touch-none border border-route bg-recess disabled:cursor-not-allowed disabled:opacity-45"
            style={{ [corner.startsWith("n") ? "top" : "bottom"]: -7, [corner.endsWith("w") ? "left" : "right"]: -7, cursor: !canEditPicture ? "not-allowed" : corner === "nw" || corner === "se" ? "nwse-resize" : "nesw-resize" }} />)}
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Legend>Crop</Legend>
        {ASPECTS.map((aspect) => <Button key={aspect.label} tone="ghost" disabled={!canEditPicture} onClick={() => setCrop(cropFromStage(aspectCrop(stageSize.width, stageSize.height, aspect.ratio), settings.rotate, settings.flipHorizontal, settings.flipVertical))}>{aspect.label}</Button>)}
        <Button tone="ghost" disabled={!canEditPicture || !settings.crop} onClick={() => onChange({ crop: null })}>Reset crop</Button>
        <span className="mx-1 h-4 w-px bg-edge" aria-hidden="true" />
        <Legend>Orientation</Legend>
        <Button tone="ghost" disabled={!canEditPicture} onClick={() => onChange({ rotate: ((settings.rotate + 90) % 360) as ConversionSettings["rotate"] })}>Rotate 90°{settings.rotate ? ` (now ${settings.rotate}°)` : ""}</Button>
        <Button tone="ghost" disabled={!canEditPicture} onClick={() => onChange({ flipHorizontal: !settings.flipHorizontal })}>{settings.flipHorizontal ? "Flipped ↔" : "Flip ↔"}</Button>
        <Button tone="ghost" disabled={!canEditPicture} onClick={() => onChange({ flipVertical: !settings.flipVertical })}>{settings.flipVertical ? "Flipped ↕" : "Flip ↕"}</Button>
        <Button tone="ghost" disabled={!canEditPicture || !edited} onClick={() => onChange({ crop: null, rotate: 0, flipHorizontal: false, flipVertical: false })}>Reset all</Button>
      </div>
      <p className="text-[11px] text-ink-faint">The crop is applied first, then rotation, then the maximum size.{result ? ` Result: ${result.width} × ${result.height} px.` : ""}</p>
    </div>}

    {timed && <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between"><Legend>Time range</Legend><span className="font-mono text-[11px] text-ink-dim" aria-live="polite">{clock(range.start)} – {clock(range.end)} · {clock(range.end - range.start)} long</span></div>
      <div ref={track} className="recess relative h-14 select-none rounded-hw-lg" onPointerDown={(event) => setScrub(secondsAt(event.clientX))}>
        {wave && <div className="absolute inset-x-0 inset-y-1 flex items-center gap-px px-px" aria-hidden="true">
          {wave.map((peak, index) => <span key={index} className="flex-1 bg-ink-faint/60" style={{ height: `${Math.max(4, peak * 100)}%` }} />)}
        </div>}
        <div className="absolute inset-y-0 bg-route/15" style={{ left: percent(range.start), width: percent(range.end - range.start) }} aria-hidden="true" />
        <div className="pointer-events-none absolute inset-y-0 w-px bg-ink" style={{ left: percent(Math.min(scrub, duration)) }} aria-hidden="true" />
        {(["start", "end"] as const).map((handle) => {
          const handleDisabled = handle === "end" && !canEditEnd;
          return <div key={handle} role="slider" tabIndex={handleDisabled ? -1 : 0} aria-disabled={handleDisabled || undefined} aria-label={handle === "start" ? "Trim start" : "Trim end"}
            aria-valuemin={handle === "start" ? 0 : range.start + MIN_TRIM_SECONDS} aria-valuemax={handle === "start" ? range.end - MIN_TRIM_SECONDS : duration}
            aria-valuenow={round(range[handle], 3)} aria-valuetext={clock(range[handle])}
            onPointerDown={(event) => { event.stopPropagation(); dragHandle(handle)(event); }} onKeyDown={keyHandle(handle)}
            className={`focus-ring absolute inset-y-0 w-2.5 -translate-x-1/2 touch-none bg-route ${handleDisabled ? "cursor-not-allowed opacity-45" : "cursor-ew-resize"}`} style={{ left: percent(range[handle]) }} />;
        })}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button tone="ghost" onClick={() => setScrub(range.start)}>Show start</Button>
        <Button tone="ghost" disabled={!canEditEnd} onClick={() => setScrub(Math.max(range.start, range.end - 0.1))}>Show end</Button>
        <Button tone="ghost" disabled={settings.startSeconds === 0 && settings.endSeconds === null} onClick={() => onChange({ startSeconds: 0, endSeconds: null })}>Use whole file</Button>
        <span className="text-[11px] text-ink-faint">Arrow keys nudge a handle by 0.1 s; hold Shift for 1 s.</span>
      </div>
    </div>}
    {problem && <p role="alert" className="text-[12px] text-danger">{problem}</p>}
  </div>;
}
