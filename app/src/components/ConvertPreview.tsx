import { useState } from "react";
import { Button } from "./Controls";
import { Legend } from "./Panel";
import { engine, type ConversionPreviewSample, type ConversionSettings, type MediaInfo } from "../lib/engine";
import { estimateFullBytes } from "../lib/conversion";
import { frameTransform, relativeOrientation } from "../lib/cropMath";
import { formatBytes } from "../lib/format";

const SAMPLE_SECONDS = 4;

/** Render a few seconds with the current settings so the result can be seen before a long job. */
export function ConvertPreview({ file, settings, outputSeconds }: { file: MediaInfo; settings: ConversionSettings; outputSeconds: number | null }) {
  const [sample, setSample] = useState<{ result: ConversionPreviewSample; recipe: string; settings: ConversionSettings } | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const recipe = JSON.stringify(settings);
  const timed = (file.durationSeconds ?? 0) > 0;

  async function render() {
    setBusy(true); setProblem(null);
    try {
      // The sample starts at the chosen start time, so it shows the part that will be kept.
      setSample({ result: await engine.conversionPreview(file.path, settings, settings.startSeconds, SAMPLE_SECONDS), recipe, settings });
    } catch (e) { setProblem(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  }

  const result = sample?.result;
  const estimate = result && result.sampleSeconds > 0 ? estimateFullBytes(result.bytes, result.sampleSeconds, outputSeconds) : null;
  const kind = result?.url.startsWith("data:video") ? "video" : result?.url.startsWith("data:audio") ? "audio" : "image";
  const delta = sample ? relativeOrientation(sample.settings, settings) : null;
  const previewTransform = delta ? frameTransform(delta.rotate, delta.flipHorizontal, delta.flipVertical, 1, 1) : undefined;
  return <div className="flex flex-col gap-3" data-testid="convert-preview">
    <div className="flex flex-wrap items-center gap-3">
      <Button tone="ghost" disabled={busy} onClick={() => void render()}>{busy ? "Rendering…" : result ? "Render again" : `Preview ${timed ? `${SAMPLE_SECONDS} seconds` : "result"}`}</Button>
      <span className="text-[11px] text-ink-faint">Uses a sample up to 960 × 540 while the queue is idle. Queued work cancels it. Recent samples are cached until the app closes.</span>
    </div>
    {problem && <p role="alert" className="text-[12px] text-danger">{problem}</p>}
    {result && <div className="flex flex-col gap-2">
      {sample?.recipe !== recipe && <p className="text-[11px] text-caution">Settings changed since this preview. Render again to update it.</p>}
      <div className="recess mx-auto flex max-w-xl justify-center overflow-hidden rounded-hw-lg p-2">
        {kind === "video" && <video src={result.url} controls loop muted className="max-h-72 max-w-full" style={previewTransform ? { transform: previewTransform } : undefined} aria-label="Preview of the converted sample" />}
        {kind === "audio" && <audio src={result.url} controls className="w-full" aria-label="Preview of the converted sample" />}
        {kind === "image" && <img src={result.url} alt="Preview of the converted sample" className="max-h-72 max-w-full object-contain" style={previewTransform ? { transform: previewTransform } : undefined} />}
      </div>
      <p data-selectable className="text-[11px] text-ink-dim">
        {[`Sample ${result.sampleSeconds.toFixed(1)} s from ${result.startSeconds.toFixed(1)} s`, formatBytes(result.bytes), result.width && result.height ? `${result.width} × ${result.height}` : null,
          result.format !== settings.format ? `shown as ${result.format.toUpperCase()}` : null].filter(Boolean).join(" · ")}
        {estimate ? ` · whole file about ${formatBytes(estimate)} (rough guess from a reduced-size sample; actual output can differ)` : ""}
      </p>
    </div>}
    <Legend className="sr-only">Preview</Legend>
  </div>;
}
