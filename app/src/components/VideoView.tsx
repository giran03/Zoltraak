import { FailureActions } from "./FailureActions";
import { SlideGroup } from "./SlideGroup";
import { DownloadDisclosure } from "./DownloadRow";
import { videoJobToSnapshot, type QueueItem } from "../lib/snapshots";
import type { RecoveryKind } from "../lib/downloadUx";
import { useEffect, useRef, useState } from "react";
import { ClearButton, Button, ErrorStrip, NumberInput, Section, SelectInput, Switch } from "./Controls";
import { Check, Discord, DownloadGlyph, FolderOpen, Pause, Play, Trash } from "./Icons";
import { Lamp, Legend } from "./Panel";
import type { ToastItem } from "./Toast";
import { TooltipGroup } from "./Tooltip";
import {
  engine,
  hasEngine,
  type VideoFormat,
  type VideoJob,
  type VideoJobState,
  type VideoMetadata,
} from "../lib/engine";
import { formatBytes, formatEta, formatRate } from "../lib/format";
import type { useVideo } from "../lib/useVideo";
import { onTabKeyDown } from "../lib/tabKeys";
import { choosePreset, type VideoPreset } from "../lib/videoPresets";

const STATE_LABEL: Record<VideoJobState, string> = {
  downloading: "DOWNLOADING",
  merging: "MERGING",
  paused: "PAUSED",
  finished: "FINISHED",
  failed: "FAILED",
};

/** Video jobs use the same signal aspects as the register. */
const STATE_LAMP: Record<VideoJobState, string | null> = {
  downloading: "bg-clear",
  merging: "bg-route",
  paused: "bg-paused",
  finished: null,
  failed: "bg-danger",
};

const STATE_INK: Record<VideoJobState, string> = {
  downloading: "text-clear",
  merging: "text-route",
  paused: "text-paused",
  finished: "text-ink-faint",
  failed: "text-danger",
};

/**
 * The one-line readout under a job title: how far along, how fast, how long
 * left. Each part is dropped when the engine cannot supply it, so a source
 * with no known size degrades to a byte count rather than showing "NaN%" or a
 * confidently wrong ETA.
 */
function describeProgress(job: VideoJob): string {
  if (job.state === "finished") {
    const checked = job.validation?.status === "verified"
      ? (job.validation.samplesDecoded ? "tracks and samples checked" : "tracks checked")
      : "verification unavailable";
    return `${formatBytes(job.downloadedBytes)} · ${checked}${job.validation?.hasAudio === false ? " · no audio" : ""}`;
  }
  const parts: string[] = [];

  if (job.totalBytes && job.totalBytes > 0) {
    const pct = Math.round(Math.min(job.downloadedBytes / job.totalBytes, 1) * 100);
    parts.push(`${formatBytes(job.downloadedBytes)} / ${formatBytes(job.totalBytes)} (${pct}%)`);
  } else {
    parts.push(formatBytes(job.downloadedBytes));
  }

  if (job.bytesPerSecond > 0) parts.push(formatRate(job.bytesPerSecond));
  if (job.etaSeconds !== null) parts.push(`${formatEta(job.etaSeconds)} left`);

  return parts.join(" · ");
}

/** `1080p`, `320kbps`, or the raw id — mirrors `VideoFormat::label` in Rust. */
function label(format: VideoFormat): string {
  const quality = format.height ? `${format.height}p`
    : format.bitrate ? `${Math.round(format.bitrate / 1000)}kbps` : format.id;
  const clip = /^clip-(\d+)-/.exec(format.id)?.[1];
  return clip ? `Clip ${clip} · ${quality}` : quality;
}

/**
 * What the row has to admit about its sound, if anything.
 *
 * `merges audio` is reassurance (two streams, one finished file). The other
 * two are warnings: a positively-silent video track the engine could not pair,
 * and a bare audio track that arrived where a video was expected. Both are
 * still offered — they may be all there is — but never unlabelled.
 */
function audioNote(format: VideoFormat): { text: string; warn: boolean } | null {
  if (format.requiresMerge) return { text: "merges audio", warn: false };
  if (format.container === "m4a") return { text: "audio only", warn: true };
  if (format.hasAudio === false) return { text: "no audio", warn: true };
  if (format.hasAudio == null) return { text: "audio unverified", warn: true };
  if (format.validation?.status !== "verified") return { text: "audio inferred", warn: false };
  return null;
}

/**
 * A video job's meter.
 *
 * One continuous bar, not a block strip. A video job has no byte-range resume
 * (see PRODUCT.md) — there are no segments to divide — so drawing it as twelve
 * cells would claim a segmentation the engine never performed, on the one
 * screen whose promise is that its numbers are real.
 */
function JobOccupancy({ job }: { job: VideoJob }) {
  const fraction =
    job.state === "merging"
      ? 1
      : job.totalBytes && job.totalBytes > 0
        ? Math.min(job.downloadedBytes / job.totalBytes, 1)
        : null;
  const fill =
    job.state === "merging" ? "bg-route" : job.state === "paused" ? "bg-paused" : "bg-clear";

  return (
    <span className="flex h-[9px] w-full" aria-hidden="true">
      <span className="block relative min-w-0 flex-1 overflow-hidden rounded-hw-sm bg-block-clear">
        <span
          className={`absolute inset-y-0 left-0 ${fill} ${
            fraction === null ? "opacity-40" : ""
          }`}
          style={{ width: `${(fraction ?? 1) * 100}%` }}
        />
      </span>
    </span>
  );
}

function FormatRow({
  format,
  activeJob,
  onDownload,
  thumbnail,
  disabled,
  recording,
}: {
  format: VideoFormat;
  activeJob?: VideoJob | null;
  onDownload: (autoDiscord?: boolean) => Promise<boolean>;
  disabled?: boolean;
  recording?: boolean;
  /** The video's poster. One per video, not per rendition — every row shows
   * the same frame, which is the point: it says what these are renditions of. */
  thumbnail?: string | null;
}) {
  const [busy, setBusy] = useState(false);
  const [success, setSuccess] = useState(false);
  const [discordBusy, setDiscordBusy] = useState(false);
  const [discordSuccess, setDiscordSuccess] = useState(false);

  const isDownloading = activeJob?.state === "downloading" || activeJob?.state === "merging";

  async function handleDownload(autoDiscord = false) {
    if (autoDiscord) {
      setDiscordBusy(true);
      setDiscordSuccess(false);
    } else {
      setBusy(true);
      setSuccess(false);
    }

    const started = await onDownload(autoDiscord);
    if (!started) { setBusy(false); setDiscordBusy(false); return; }

    if (autoDiscord) {
      setDiscordBusy(false);
      setDiscordSuccess(true);
      setTimeout(() => setDiscordSuccess(false), 2000);
    } else {
      setBusy(false);
      setSuccess(true);
      setTimeout(() => setSuccess(false), 2000);
    }
  }

  const fraction =
    activeJob?.totalBytes && activeJob.totalBytes > 0
      ? Math.min(activeJob.downloadedBytes / activeJob.totalBytes, 1)
      : null;

  const note = audioNote(format);

  const detail =
    [
      format.bitrate ? `${Math.round(format.bitrate / 1000)} kbps` : null,
      format.filesize ? formatBytes(format.filesize) : null,
      format.videoCodec,
    ]
      .filter(Boolean)
      .join(" · ") || "—";

  return (
    <div className="flex flex-col border-b border-edge px-4 py-2.5 transition-colors last:border-b-0 hover:bg-panel">
      <div className="flex items-center gap-3">
        {thumbnail ? (
          <img
            src={thumbnail}
            alt=""
            loading="lazy"
            className="h-9 w-16 shrink-0 rounded-hw border border-edge object-cover"
          />
        ) : (
          <div className="recess flex h-9 w-16 shrink-0 items-center justify-center rounded-hw text-ink-faint">
            <Play className="size-3.5" />
          </div>
        )}

        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2.5">
            <p className="text-[13px] text-ink">{label(format)}</p>
            <Legend>{format.container}</Legend>
            {note && (
              <span
                className={`legend rounded-hw border px-1.5 py-[1px] text-[11px] ${
                  note.warn
                    ? "border-caution/50 bg-caution/12 text-caution"
                    : "border-edge-strong text-ink-faint"
                }`}
              >
                {note.text}
              </span>
            )}
          </div>
          <p data-measure className="describer mt-1 truncate text-[11px] text-ink-dim">
            {detail}
          </p>
        </div>

        <TooltipGroup side="top" className="flex items-center gap-1.5">
          <Button
            onClick={() => void handleDownload(false)}
            disabled={disabled || isDownloading || busy || success || discordBusy}
            tone={success ? "ghost" : "primary"}
            className="flex items-center gap-1.5"
          >
            {success ? <Check className="size-3.5" /> : <DownloadGlyph className="size-3.5" />}
            {isDownloading
              ? activeJob.state === "merging"
                ? "merging…"
                : fraction !== null
                  ? `${Math.round(fraction * 100)}%`
                  : "downloading…"
              : success
                ? "entered"
                : busy
                  ? "starting…"
                  : recording ? "record" : "download"}
          </Button>

          <button
            type="button"
            onClick={() => void handleDownload(true)}
            disabled={disabled || isDownloading || busy || discordBusy || discordSuccess}
            aria-label="Download and auto-send to Discord"
            data-tooltip="Download and auto-send to Discord"
            className={`t-tt-trigger focus-ring flex h-[31px] w-[31px] shrink-0 items-center justify-center rounded-hw border transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-45 ${
              discordSuccess
                ? "border-route bg-route/15 text-route"
                : "border-edge-strong bg-face text-ink-dim hover:border-discord hover:bg-discord/10 hover:text-discord"
            }`}
          >
            {discordSuccess ? <Check className="size-3.5" /> : <Discord className="size-4" />}
          </button>
        </TooltipGroup>
      </div>

      {isDownloading && activeJob && (
        <div className="mt-2.5">
          <JobOccupancy job={activeJob} />
        </div>
      )}
    </div>
  );
}

/**
 * Paste-a-link video capture: analyze a page or direct media/manifest URL,
 * pick a quality, and track the resulting job through download (and merge,
 * for formats whose audio arrives as a separate stream) to a finished file.
 */
export function VideoView({
  video,
  prefill,
  onAddToast,
  onNavigateToDownloads,
  onRemove,
  onRecover,
  deleteFilesOnRemove = false,
}: {
  video: ReturnType<typeof useVideo>;
  prefill?: {
    url: string;
    headers: [string, string][];
    autoDownload?: boolean;
    title?: string;
    directory?: string;
    useFallback?: boolean;
    nonce: number;
  } | null;
  onAddToast?: (toast: ToastItem) => void;
  onNavigateToDownloads?: () => void;
  onRemove?: (job: VideoJob, deleteFile?: boolean) => void;
  onRecover?: (item: QueueItem, kind: RecoveryKind) => Promise<void>;
  deleteFilesOnRemove?: boolean;
}) {
  const [url, setUrl] = useState("");
  const [preset, setPreset] = useState<VideoPreset>(() => {
    try { const value = localStorage.getItem("zoltraak.videoPreset"); return value === "balanced" || value === "compatible" ? value : "best"; } catch { return "best"; }
  });
  const [subtitleIndex, setSubtitleIndex] = useState("");
  const [recordLive, setRecordLive] = useState(false);
  const [recordSeconds, setRecordSeconds] = useState("300");
  const [presetBusy, setPresetBusy] = useState(false);
  const seconds = Number(recordSeconds);
  const validDuration = Number.isInteger(seconds) && seconds >= 5 && seconds <= 1800;
  const selectedSubtitle = subtitleIndex === "" ? undefined : video.metadata?.subtitles?.[Number(subtitleIndex)];
  const downloadOptions = { subtitle: selectedSubtitle, liveSeconds: recordLive ? seconds : undefined };
  const canRecord = (format: VideoFormat) => !format.requiresMerge && /\.m3u8(?:[?#]|$)/i.test(format.videoUrl ?? "");
  const presetFormat = choosePreset(video.metadata?.formats ?? [], preset);
  useEffect(() => { setSubtitleIndex(""); setRecordLive(false); }, [video.metadata]);
  const [jobTab, setJobTab] = useState<"active" | "completed" | "failed">("active");
  const inputRef = useRef<HTMLInputElement>(null);
  const analysisWorkflow = useRef(0);
  const [prefillHeaders, setPrefillHeaders] = useState<{
    url: string;
    headers: [string, string][];
  } | null>(null);

  async function performAnalyze(
    targetUrl: string,
    requestHeaders?: [string, string][],
    titleOverride?: string,
  ): Promise<VideoMetadata | null> {
    const workflow = ++analysisWorkflow.current;
    // The engine owns platform retries and cache busting. Re-analyzing an X
    // URL here could discard a successful result or mutate its signed query.
    const result = await video.analyze(targetUrl, requestHeaders, { titleOverride });
    return workflow === analysisWorkflow.current ? result : null;
  }

  // A prefill from the extension fills the box with the URL and analyzes it
  // automatically. When it came from the floating "Download this video" button
  // (`autoDownload`), also start the best-quality result straight away — no
  // format pick. The X.com right-click menu leaves the pick to the user.
  // `title` is the source page's document.title — used only as a fallback
  // filename when the extractor returned a raw CDN link with no title of its
  // own (see useVideo's "direct" check). Keyed on the nonce so re-sending the
  // same URL re-fills it.
  useEffect(() => {
    if (!prefill?.url) return;
    const targetUrl = prefill.url;
    const autoDownload = prefill.autoDownload;
    setUrl(targetUrl);
    setPrefillHeaders({ url: targetUrl, headers: prefill.headers });
    setTimeout(() => {
      if (inputRef.current) {
        inputRef.current.focus();
        inputRef.current.select();
      }
    }, 50);
    void (async () => {
      const meta = prefill.useFallback ? await video.analyze(targetUrl, prefill.headers, { useFallback: true, titleOverride: prefill.title }) : await performAnalyze(targetUrl, prefill.headers, prefill.title);
      if (autoDownload && meta && meta.formats.length > 0) {
        const ok = await video.downloadBest(meta, prefill.headers);
        if (ok && onNavigateToDownloads) {
          setUrl("");
          video.clearMetadata();
          video.clearError();
          setPrefillHeaders(null);
          onNavigateToDownloads();
        } else {
          setJobTab("active");
        }
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prefill?.nonce]);

  const activeJobs = video.jobs.filter(
    (j) => j.state === "downloading" || j.state === "merging" || j.state === "paused",
  );
  const completedJobs = video.jobs.filter((j) => j.state === "finished");
  const failedJobs = video.jobs.filter((j) => j.state === "failed");

  const currentJobs =
    jobTab === "completed" ? completedJobs : jobTab === "failed" ? failedJobs : activeJobs;

  const TABS: { id: typeof jobTab; label: string; count: number }[] = [
    { id: "active", label: "Active", count: activeJobs.length },
    { id: "completed", label: "Finished", count: completedJobs.length },
    { id: "failed", label: "Failed", count: failedJobs.length },
  ];

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = url.trim();
    if (!trimmed || video.analyzing) return;
    const headers = prefillHeaders?.url === trimmed ? prefillHeaders.headers : undefined;
    await performAnalyze(trimmed, headers);
  }

  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-7 px-6 py-6">
      <Section
        title="Analyze a source"
        aside={url.length > 0 ? <ClearButton disabled={video.analyzing} onClick={() => {
          setUrl("");
          video.clearMetadata();
          video.clearError();
          setPrefillHeaders(null);
          inputRef.current?.focus();
        }} /> : undefined}
        description="Analyze a video page, direct file, or HLS/DASH stream. Choose a discovered format before downloading."
      >
        <form autoComplete="off" onSubmit={(e) => void submit(e)} className="flex items-end gap-2">
          <input
            ref={inputRef}
            value={url}
            onChange={(e) => {
              setUrl(e.target.value);
              if (!e.target.value.trim()) {
                ++analysisWorkflow.current;
                video.clearMetadata();
                setPrefillHeaders(null);
              }
            }}
            placeholder="https://example.com/watch?v=…  or a direct .mp4 / .m3u8 / .mpd link"
            aria-label="Video page or media URL" autoComplete="off"
            spellCheck={false}
            className="recess focus-ring min-w-0 flex-1 rounded-hw px-2.5 py-1.5 font-mono text-[12px] text-ink outline-none placeholder:text-ink-faint"
          />
          <Button type="submit" disabled={video.analyzing}>
            {video.analyzing ? "analyzing…" : "analyze"}
          </Button>
        </form>

        {video.error && <ErrorStrip message={video.error} onDismiss={video.clearError} />}
        {url.trim() && (
          <details className="mt-3" open={video.error ? true : undefined}>
            <summary className="focus-ring cursor-pointer text-[12px] text-ink-dim">Optional yt-dlp extractor</summary>
          <div className="mt-2 flex flex-wrap items-center gap-3">
            <Button disabled={video.analyzing} onClick={() => void video.analyze(url.trim(), prefillHeaders?.url === url.trim() ? prefillHeaders.headers : undefined, { useFallback: true })}>Retry with fallback</Button>
            <p className="text-[12px] text-ink-dim">Instagram retries use your shared sign-in first. Install yt-dlp separately if needed for public links; browser credentials stay in Zoltraak.</p>
          </div>
          </details>
        )}
      </Section>

      {prefill?.directory && <p className="break-all text-[12px] text-ink">This download will use: {prefill.directory}</p>}
      {video.metadata && (
        <Section
          title="Formats"
          aside={
            <Legend>
              {video.metadata.formats.length}{" "}
              {video.metadata.formats.length === 1 ? "format" : "formats"}
            </Legend>
          }
        >
          <div className="border border-edge bg-face">
            <div className="flex items-center gap-3 border-b border-edge bg-recess px-4 py-2.5">
              {video.metadata.thumbnail ? (
                <img
                  src={video.metadata.thumbnail}
                  alt=""
                  className="h-11 w-[74px] shrink-0 rounded-hw border border-edge object-cover"
                />
              ) : (
                <div className="plate flex size-11 shrink-0 items-center justify-center rounded-hw text-ink-faint">
                  <Play className="size-4" />
                </div>
              )}
              <div className="min-w-0 flex-1">
                <p className="truncate text-[13px] text-ink">{video.metadata.title}</p>
                <p
                  data-selectable
                  className="describer mt-0.5 truncate text-[11px] text-ink-dim"
                >
                  {video.metadata.webpageUrl}
                </p>
              </div>
            </div>

            <div className="flex flex-col gap-4 border-b border-edge px-4 py-4">
              <fieldset className="flex flex-wrap items-center gap-x-5 gap-y-2">
                <legend className="mb-2 text-[12px] text-ink-dim">Download preset</legend>
                {([["best", "Best available"], ["balanced", "Up to 720p"], ["compatible", "H.264 / AAC"]] as const).map(([value, text]) => (
                  <label key={value} className="flex items-center gap-2 text-[12px] text-ink">
                    <input className="focus-ring accent-accent" type="radio" name="video-preset" value={value} checked={preset === value} onChange={() => { setPreset(value); try { localStorage.setItem("zoltraak.videoPreset", value); } catch {} }} />{text}
                  </label>
                ))}
                <Button disabled={!presetFormat || presetBusy || recordLive && (!validDuration || !canRecord(presetFormat!))} onClick={() => { if (!presetFormat) return; setPresetBusy(true); void video.download(presetFormat, false, downloadOptions, prefill?.directory).then(ok => { if (ok) setJobTab("active"); }).finally(() => setPresetBusy(false)); }}>
                  {presetBusy ? "starting…" : recordLive ? "Record preset" : "Download preset"}
                </Button>
              </fieldset>
              <p className="text-[12px] text-ink-dim" aria-live="polite">{presetFormat ? `Selected: ${label(presetFormat)} · ${presetFormat.container}` : "No discovered format matches this preset. Choose a format below."}</p>
              <div className="flex flex-wrap items-end gap-5">
                <div className="flex min-w-44 flex-col gap-1.5 text-[12px] text-ink-dim"><span>Subtitles</span>
                  <SelectInput ariaLabel="Subtitles" name="subtitle" value={subtitleIndex} onChange={setSubtitleIndex} options={[
                    { value: "", label: "None" },
                    ...(video.metadata.subtitles ?? []).map((s, i) => ({ value: String(i), label: `${s.label || s.language}${s.automatic ? " (automatic)" : ""} · ${s.extension}`, badge: `.${s.extension}` })),
                  ]} />
                </div>
                <div className="flex items-center gap-3 py-1.5 text-[12px] text-ink"><Switch label="Record live HLS" checked={recordLive} disabled={!video.metadata.formats.some(canRecord)} onChange={setRecordLive} /><span>Record live HLS</span></div>
                {recordLive && <div className="flex flex-col gap-1.5 text-[12px] text-ink-dim"><span>Recording seconds</span>
                  <NumberInput name="record-seconds" ariaLabel="Recording seconds" min={5} max={1800} step={1} value={recordSeconds} aria-invalid={!validDuration} aria-describedby="live-help" onChange={setRecordSeconds} className="w-28" />
                </div>}
              </div>
              <p id="live-help" className="text-[12px] text-ink-dim">{recordLive ? "5–1800 seconds, starting near the live edge. The deadline continues while paused; split audio is unavailable. Segment boundaries can extend the recording." : selectedSubtitle ? "Subtitles are saved beside the video as a separate text file." : "Subtitles appear when a direct caption track is discovered."}</p>
            </div>
            {video.metadata.formats.map((format) => {
              const activeJob = activeJobs.find(
                (j) =>
                  j.formatId === format.id ||
                  (activeJobs.length === 1 && j.webpageUrl === video.metadata?.webpageUrl),
              );
              return (
                <FormatRow
                  key={format.id}
                  format={format}
                  activeJob={activeJob}
                  thumbnail={format.downloadOptions?.thumbnail ?? video.metadata?.thumbnail}
                  disabled={recordLive && (!validDuration || !canRecord(format))}
                  recording={recordLive}
                  onDownload={async (autoDiscord) => {
                    const ok = await video.download(format, autoDiscord, downloadOptions, prefill?.directory);
                    if (ok) {
                      if (autoDiscord) {
                        if (hasEngine()) {
                          try {
                            const status = await engine.discordStatus();
                            if (!status.connected) {
                              onAddToast?.({
                                id: `discord-warn-${Date.now()}`,
                                type: "error",
                                title: "Discord not connected",
                                message:
                                  "Video download started, but Discord delivery is not configured yet. Connect it in the Discord tab.",
                              });
                            }
                          } catch {}
                        }
                        onAddToast?.({
                          id: `discord-auto-${Date.now()}`,
                          type: "info",
                          title: "Queued with Discord auto-delivery",
                          message:
                            "When this video finishes downloading and merging, it will be automatically delivered to Discord.",
                        });
                      }

                      if (onNavigateToDownloads) {
                        setUrl("");
                        video.clearMetadata();
                        video.clearError();
                        setPrefillHeaders(null);
                        onNavigateToDownloads();
                      }
                    }
                    return ok;
                  }}
                />
              );
            })}
          </div>
        </Section>
      )}

      <Section
        title="Video jobs"
        aside={
          <div className="flex flex-wrap items-center gap-2">
            <TooltipGroup side="bottom">
              <button
                type="button"
                onClick={() => void engine.revealFolder()}
                data-tooltip="Open video download folder (Ctrl + E)"
                className="t-tt-trigger focus-ring legend flex items-center gap-1.5 rounded-hw border border-edge-strong px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-face hover:text-ink"
              >
                <FolderOpen className="size-3.5" />
                Open folder
              </button>
            </TooltipGroup>
            <SlideGroup
              role="tablist"
              aria-label="Video job state"
              onKeyDown={onTabKeyDown}
              className="flex overflow-hidden rounded-hw border border-edge-strong"
            >
              {TABS.map((tab) => (
                <button
                  key={tab.id}
                  type="button"
                  role="tab"
                  aria-selected={jobTab === tab.id}
                  tabIndex={jobTab === tab.id ? 0 : -1}
                  onClick={() => setJobTab(tab.id)}
                  className={`focus-ring legend border-r border-edge px-2.5 py-1 text-[11px] transition-colors last:border-r-0 ${
                    jobTab === tab.id
                      ? "text-on-signal"
                      : "text-ink-dim hover:bg-face hover:text-ink"
                  }`}
                >
                  {tab.label} {tab.count}
                </button>
              ))}
            </SlideGroup>
          </div>
        }
      >
        {currentJobs.length === 0 ? (
          <div className="flex flex-col items-center gap-3 border border-edge bg-face px-6 py-12 text-center">
            <span className="h-px w-24 bg-edge" />
            <Legend className="text-ink-dim">
              {jobTab === "completed"
                ? "nothing finished yet"
                : jobTab === "failed"
                  ? "no faults"
                  : "no jobs running"}
            </Legend>
          </div>
        ) : (
          <div className="border border-edge bg-face">
            {currentJobs.map((job) => {
              const running =
                job.state === "downloading" || job.state === "merging" || job.state === "paused";

              return (
                <div
                  key={job.id}
                  className="flex flex-wrap items-center gap-3 border-b border-edge px-4 py-2.5 transition-colors last:border-b-0 hover:bg-panel"
                >
                  <Lamp fill={STATE_LAMP[job.state]} size={9} />

                  <div className="min-w-0 flex-1">
                    <div className="flex items-baseline gap-2.5">
                      <p className="truncate text-[13px] text-ink">{job.title}</p>
                      <Legend className={STATE_INK[job.state]}>{STATE_LABEL[job.state]}</Legend>
                    </div>
                    <p
                      data-selectable
                      data-measure
                      className="describer mt-1 truncate text-[11px] text-ink-dim"
                    >
                      {job.state === "failed"
                        ? (job.error ?? "failed")
                        : job.state === "merging"
                          ? (job.path ? job.path : "Merging audio and video streams…")
                          : job.state === "downloading"
                            ? describeProgress(job)
                            : job.state === "paused"
                              ? `${describeProgress(job)} · paused`
                              : `${describeProgress(job)}${job.path ? ` · ${job.path}` : ""}`}
                    </p>
                    {running && (
                      <div className="mt-2">
                        <JobOccupancy job={job} />
                      </div>
                    )}
                  </div>

                  {/* Visible at rest, like every other control in the app: a
                      button that appears only under the pointer is
                      undiscoverable and unreachable from the keyboard. */}
                  <TooltipGroup side="top" className="flex shrink-0 items-center gap-0.5">
                    {running && (
                      <JobAction
                        label={`Pause ${job.title}`}
                        tooltip="Pause"
                        onClick={() => void video.pause(job.id)}
                      >
                        <Pause className="size-3.5" />
                      </JobAction>
                    )}
                    {(job.state === "paused" || job.state === "failed") && (
                      <JobAction
                        label={`Resume ${job.title}`}
                        tooltip="Resume"
                        onClick={() => void video.resume(job.id)}
                      >
                        <Play className="size-3.5" />
                      </JobAction>
                    )}
                    {job.path && (
                      <>
                        {job.state === "finished" && (
                        <JobAction
                          label={`Send ${job.title} to Discord`}
                          tooltip="Send to Discord"
                          onClick={() => {
                            if (!hasEngine()) return;
                            void engine
                              .discordSend([job.id])
                              .then(() => {
                                onAddToast?.({
                                  id: `discord-send-${Date.now()}`,
                                  type: "info",
                                  title: "Queued for Discord",
                                  message: `${job.title} queued for Discord delivery.`,
                                });
                              })
                              .catch((err: unknown) => {
                                onAddToast?.({
                                  id: `discord-err-${Date.now()}`,
                                  type: "error",
                                  title: "Could not send to Discord",
                                  message: err instanceof Error ? err.message : String(err),
                                });
                              });
                          }}
                        >
                          <Discord className="size-3.5" />
                        </JobAction>
                        )}
                        <JobAction
                          label="Show in folder"
                          tooltip="Show in folder"
                          // By id, not by path: the engine holds the authoritative
                          // location and selects the file itself, the same way the
                          // downloads list does.
                          onClick={() => void engine.reveal(job.id)}
                        >
                          <FolderOpen className="size-3.5" />
                        </JobAction>
                      </>
                    )}
                    <JobAction
                      label={deleteFilesOnRemove ? `Delete ${job.title} and local file` : `Remove ${job.title} from list`}
                      tooltip={deleteFilesOnRemove ? "Delete entry and file" : "Remove from list"}
                      onClick={() => onRemove ? onRemove(job) : void video.remove(job.id, deleteFilesOnRemove)}
                    >
                      <Trash className="size-3.5" />
                    </JobAction>
                  </TooltipGroup>
                  {job.state === "failed" && <div className="basis-full"><FailureActions item={videoJobToSnapshot(job)} onRetry={() => video.resume(job.id)} onRecover={onRecover} /></div>}
                  <DownloadDisclosure item={videoJobToSnapshot(job)} onRemoveEntry={onRemove ? () => onRemove(job, false) : undefined} />
                </div>
              );
            })}
          </div>
        )}
      </Section>
    </div>
  );
}

function JobAction({
  label: name,
  tooltip,
  onClick,
  children,
}: {
  label: string;
  tooltip?: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={name}
      data-tooltip={tooltip ?? name}
      onClick={onClick}
      className="t-tt-trigger focus-ring flex size-7 shrink-0 items-center justify-center rounded-hw border border-transparent text-ink-dim transition-colors hover:border-edge hover:bg-recess hover:text-ink"
    >
      {children}
    </button>
  );
}
