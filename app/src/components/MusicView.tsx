import { FailureActions } from "./FailureActions";
import { SlideGroup } from "./SlideGroup";
import { DownloadDisclosure } from "./DownloadRow";
import { musicJobToSnapshot, type QueueItem } from "../lib/snapshots";
import type { RecoveryKind } from "../lib/downloadUx";
import { useEffect, useMemo, useRef, useState } from "react";
import { ClearButton, Button, ErrorStrip, Field, Section, SelectInput, Switch, TextInput } from "./Controls";
import { Check, Discord, DownloadGlyph, FolderOpen, Note, Pause, Play, RotateCcw, Trash } from "./Icons";
import { Lamp, Legend } from "./Panel";
import { SaveIndicator } from "./SaveIndicator";
import type { ToastItem } from "./Toast";
import { TooltipGroup } from "./Tooltip";
import {
  engine,
  hasEngine,
  type AudioFormat,
  type FolderMode,
  type MusicJob,
  type MusicJobState,
  type MusicService,
  type MusicSourceKind,
  type MusicTrack,
} from "../lib/engine";
import { onTabKeyDown } from "../lib/tabKeys";
import { formatBytes, formatEta, formatRate } from "../lib/format";
import { formatClock, isWaitingForSession, linksIn, trackLine, type MusicGroup, type useMusic } from "../lib/useMusic";

/**
 * The Music destination: a catalogue link in, tagged audio files out.
 *
 * Three regions, top to bottom. The entry: a link or a search. The output
 * panel: what the files become, saved as the default as it is edited. The
 * tracks: every job, grouped by the playlist or album it came from.
 *
 * What this screen is honest about, because the engine is: Spotify, Apple
 * Music and Deezer hand over a catalogue entry and no audio, so the file the
 * user gets is the recording matched on YouTube Music — the row shows which.
 */

const SERVICE_LABEL: Record<MusicService, string> = {
  spotify: "Spotify",
  apple_music: "Apple Music",
  deezer: "Deezer",
  youtube: "YouTube",
  youtube_music: "YouTube Music",
  soundcloud: "SoundCloud",
  bandcamp: "Bandcamp",
  direct: "direct link",
  search: "search",
};

const KIND_LABEL: Record<MusicSourceKind, string> = {
  track: "track",
  album: "album",
  playlist: "playlist",
  artist: "artist",
  search: "search results",
};

const STATE_LABEL: Record<MusicJobState, string> = {
  queued: "QUEUED",
  resolving: "RESOLVING",
  downloading: "DOWNLOADING",
  converting: "CONVERTING",
  finished: "FINISHED",
  failed: "FAILED",
  skipped: "ON DISK",
  paused: "PAUSED",
};

/** Music jobs use the register's signal aspects. Resolving and converting are
 * both "working", drawn in route blue like a merge. */
const STATE_LAMP: Record<MusicJobState, string | null> = {
  queued: "bg-caution",
  resolving: "bg-route",
  downloading: "bg-clear",
  converting: "bg-route",
  finished: null,
  failed: "bg-danger",
  skipped: null,
  paused: "bg-paused",
};

const STATE_INK: Record<MusicJobState, string> = {
  queued: "text-caution",
  resolving: "text-route",
  downloading: "text-clear",
  converting: "text-route",
  finished: "text-ink-faint",
  failed: "text-danger",
  skipped: "text-ink-faint",
  paused: "text-paused",
};

const FORMATS: { value: AudioFormat; label: string; group?: string; badge?: string; detail?: string }[] = [
  { value: "best", label: "Best available (no re-encode)", group: "Source" },
  { value: "mp3", label: "MP3", group: "Lossy", badge: ".mp3" },
  { value: "m4a", label: "AAC (.m4a)", group: "Lossy", badge: ".m4a" },
  { value: "opus", label: "Opus", group: "Lossy", badge: ".opus" },
  { value: "ogg", label: "Ogg Vorbis", group: "Lossy", badge: ".ogg" },
  { value: "flac", label: "FLAC (lossless container)", group: "Lossless", badge: ".flac" },
  { value: "alac", label: "ALAC (lossless container, .m4a)", group: "Lossless", badge: ".m4a", detail: "lossless container" },
  { value: "wav", label: "WAV (uncompressed)", group: "Lossless", badge: ".wav" },
];

const BITRATES: Record<AudioFormat, number[]> = {
  best: [],
  mp3: [128, 192, 256, 320],
  m4a: [128, 192, 256],
  opus: [96, 128, 160, 192],
  ogg: [128, 192, 256],
  flac: [],
  alac: [],
  wav: [],
};

const FOLDER_MODES: { value: FolderMode; label: string }[] = [
  { value: "collection", label: "Folder named after the playlist or album" },
  { value: "custom", label: "Folder with a name I choose" },
  { value: "none", label: "No folder — straight into the music folder" },
];

const PLACEHOLDERS = "{title} {artist} {artists} {album} {album_artist} {track} {track:02} {disc} {year}";
const MUSIC_INPUT_PLACEHOLDER = "https://open.spotify.com/playlist/…   ·   https://music.apple.com/…/album/…   ·   or an artist and song to search";

const ACTIVE_STATES: MusicJobState[] = ["queued", "resolving", "downloading", "converting", "paused"];

function describeJob(job: MusicJob): string {
  if (isWaitingForSession(job)) {
    return "YouTube served only a preview. Share a signed-in YouTube session and this retries itself.";
  }
  if (job.state === "failed") return job.error ?? "failed";
  if (job.state === "finished" || job.state === "skipped") {
    return job.path ?? (job.totalBytes ? formatBytes(job.totalBytes) : "");
  }
  const parts: string[] = [];
  if (job.state === "downloading") {
    if (job.totalBytes && job.totalBytes > 0) {
      const pct = Math.round(Math.min(job.downloadedBytes / job.totalBytes, 1) * 100);
      parts.push(`${formatBytes(job.downloadedBytes)} / ${formatBytes(job.totalBytes)} (${pct}%)`);
    } else if (job.downloadedBytes > 0) {
      parts.push(formatBytes(job.downloadedBytes));
    }
    if (job.bytesPerSecond > 0) parts.push(formatRate(job.bytesPerSecond));
    if (job.etaSeconds !== null) parts.push(`${formatEta(job.etaSeconds)} left`);
  }
  if (job.stage && job.state !== "downloading") parts.push(job.stage);
  if (job.state === "paused") parts.push("paused");
  if (job.state === "queued" && parts.length === 0) parts.push("waiting for a slot");
  return parts.join(" · ");
}

/** A continuous bar, as the video view draws: there are no segments to show. */
function JobBar({ job }: { job: MusicJob }) {
  const fraction =
    job.state === "converting"
      ? 1
      : job.state === "downloading" && job.totalBytes && job.totalBytes > 0
        ? Math.min(job.downloadedBytes / job.totalBytes, 1)
        : null;
  const fill =
    job.state === "converting" || job.state === "resolving"
      ? "bg-route"
      : job.state === "paused"
        ? "bg-danger"
        : "bg-clear";
  return (
    <span className="flex h-[7px] w-full" aria-hidden="true">
      <span className="relative block min-w-0 flex-1 overflow-hidden rounded-hw-sm bg-block-clear">
        <span
          className={`absolute inset-y-0 left-0 ${fill} ${fraction === null ? "opacity-40" : ""}`}
          style={{ width: `${(fraction ?? 1) * 100}%` }}
        />
      </span>
    </span>
  );
}

function RowAction({
  label,
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
      aria-label={label}
      data-tooltip={tooltip ?? label}
      onClick={onClick}
      className="t-tt-trigger focus-ring flex size-7 shrink-0 items-center justify-center rounded-hw border border-transparent text-ink-dim transition-colors hover:border-edge hover:bg-recess hover:text-ink"
    >
      {children}
    </button>
  );
}

function Cover({ url, size = "size-9" }: { url: string | null; size?: string }) {
  const [broken, setBroken] = useState(false);
  if (!url || broken) {
    return (
      <div className={`recess flex ${size} shrink-0 items-center justify-center rounded-hw text-ink-faint`}>
        <Note className="size-4" />
      </div>
    );
  }
  return (
    <img
      src={url}
      alt=""
      loading="lazy"
      onError={() => setBroken(true)}
      className={`${size} shrink-0 rounded-hw border border-edge object-cover`}
    />
  );
}

/** One track of a resolved source. The whole row is the checkbox. */
function TrackRow({
  track,
  index,
  selected,
  onToggle,
}: {
  track: MusicTrack;
  index: number;
  selected: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={selected}
      aria-label={trackLine(track)}
      onClick={onToggle}
      className={`focus-ring flex w-full items-center gap-3 border-b border-edge px-4 py-2 text-left transition-colors last:border-b-0 ${
        selected ? "bg-face" : "hover:bg-face/60"
      }`}
    >
      <span
        aria-hidden="true"
        className={`flex size-[15px] shrink-0 items-center justify-center rounded-hw border transition-colors ${
          selected ? "border-route bg-route text-on-signal" : "border-edge-strong bg-recess text-transparent"
        }`}
      >
        <Check className="size-3" />
      </span>
      <span data-measure className="describer w-6 shrink-0 text-right text-[11px] text-ink-faint">
        {track.trackNumber ?? index + 1}
      </span>
      <Cover url={track.coverUrl} size="size-8" />
      <span className="min-w-0 flex-1">
        <span className="flex items-baseline gap-2">
          <span className={`truncate text-[13px] ${track.unavailable ? "text-ink-dim" : "text-ink"}`}>{track.title}</span>
          {track.explicit && <Legend className="text-ink-faint">E</Legend>}
          {track.unavailable && <span className="legend shrink-0 text-[11px] text-caution">{track.unavailable}</span>}
        </span>
        <span className="describer mt-0.5 block truncate text-[11px] text-ink-dim">
          {track.artists.join(", ") || "unknown artist"}
          {track.album ? ` · ${track.album}` : ""}
          {track.year ? ` · ${track.year}` : ""}
        </span>
      </span>
      <span data-measure className="describer w-12 shrink-0 text-right text-[11px] text-ink-dim">
        {formatClock(track.duration)}
      </span>
    </button>
  );
}

function GroupHeader({
  group,
  music,
  onAddToast,
}: {
  group: MusicGroup;
  music: ReturnType<typeof useMusic>;
  onAddToast?: (toast: ToastItem) => void;
}) {
  const done = group.jobs.filter((j) => j.state === "finished" || j.state === "skipped").length;
  const waiting = group.jobs.filter(isWaitingForSession).length;
  const failed = group.jobs.filter((j) => j.state === "failed").length - waiting;
  const total = group.jobs.length;
  const directory = group.jobs[0]?.directory;
  const finishedIds = group.jobs.filter((j) => j.path).map((j) => j.id);

  return (
    <div className="flex items-center gap-3 border-b border-edge bg-recess px-4 py-2">
      <Cover url={group.jobs[0]?.track.coverUrl ?? null} size="size-8" />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px] text-ink">{group.title}</p>
        <p data-measure className="describer mt-0.5 truncate text-[11px] text-ink-dim">
          {done} of {total} on disk{failed > 0 ? ` · ${failed} failed` : ""}
          {waiting > 0 ? ` · ${waiting} waiting for a YouTube sign-in` : ""}
          {directory ? ` · ${directory}` : ""}
        </p>
      </div>
      <TooltipGroup side="top" className="flex shrink-0 items-center gap-0.5">
        {failed > 0 && group.collectionId && (
          <RowAction
            label="Retry the failed tracks"
            tooltip="Retry failed tracks"
            onClick={() => void music.retryCollection(group.collectionId!)}
          >
            <RotateCcw className="size-3.5" />
          </RowAction>
        )}
        {finishedIds.length > 0 && hasEngine() && (
          <RowAction
            label="Send the finished tracks to Discord"
            tooltip="Send finished to Discord"
            onClick={() => {
              void engine
                .discordSend(finishedIds)
                .then((count) =>
                  onAddToast?.({
                    id: `discord-music-${Date.now()}`,
                    type: "info",
                    title: count === 1 ? "Queued for Discord" : `${count} queued for Discord`,
                    message: "Large files are re-encoded to fit before they are sent.",
                  }),
                )
                .catch((e: unknown) =>
                  onAddToast?.({
                    id: `discord-music-err-${Date.now()}`,
                    type: "error",
                    title: "Could not send to Discord",
                    message: e instanceof Error ? e.message : String(e),
                  }),
                );
            }}
          >
            <Discord className="size-3.5" />
          </RowAction>
        )}
        {directory && hasEngine() && (
          <RowAction
            label="Open the folder"
            tooltip="Open folder"
            onClick={() => void engine.revealFolder(directory)}
          >
            <FolderOpen className="size-3.5" />
          </RowAction>
        )}
        {group.collectionId && (
          <RowAction
            label="Remove every track of this collection from the list"
            tooltip="Remove collection"
            onClick={() => void music.removeCollection(group.collectionId!)}
          >
            <Trash className="size-3.5" />
          </RowAction>
        )}
      </TooltipGroup>
    </div>
  );
}

function JobRow({
  job,
  inCollection,
  music,
  onAddToast,
  onRemove,
  onRecover,
  deleteFilesOnRemove = false,
}: {
  job: MusicJob;
  inCollection: boolean;
  music: ReturnType<typeof useMusic>;
  onAddToast?: (toast: ToastItem) => void;
  onRemove?: (job: MusicJob, deleteFile?: boolean) => void;
  onRecover?: (item: QueueItem, kind: RecoveryKind) => Promise<void>;
  deleteFilesOnRemove?: boolean;
}) {
  const working = ["resolving", "downloading", "converting"].includes(job.state);
  const holdable = working || job.state === "queued";
  const resumable = job.state === "paused" || job.state === "failed";
  const waiting = isWaitingForSession(job);
  const matched =
    job.matchedTitle && job.track.service !== "youtube" && job.track.service !== "youtube_music"
      ? `matched: ${job.matchedTitle}`
      : null;

  return (
    <div className="flex flex-wrap items-center gap-3 border-b border-edge px-4 py-2 transition-colors last:border-b-0 hover:bg-panel">
      <Lamp fill={waiting ? "bg-caution" : STATE_LAMP[job.state]} size={9} />
      {inCollection ? (
        <span data-measure className="describer w-6 shrink-0 text-right text-[11px] text-ink-faint">
          {job.position}
        </span>
      ) : (
        <Cover url={job.track.coverUrl} size="size-8" />
      )}
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2.5">
          <p className="truncate text-[13px] text-ink" title={job.track.sourceUrl}>
            {trackLine(job.track)}
          </p>
          <Legend className={waiting ? "text-caution" : STATE_INK[job.state]}>
            {waiting ? "WAITING FOR SIGN-IN" : STATE_LABEL[job.state]}
          </Legend>
          <Legend>{job.format}</Legend>
        </div>
        <p data-selectable data-measure className="describer mt-0.5 truncate text-[11px] text-ink-dim">
          {describeJob(job)}
          {matched && job.state !== "failed" ? ` · ${matched}` : ""}
        </p>
        {(working || job.state === "paused") && (
          <div className="mt-1.5">
            <JobBar job={job} />
          </div>
        )}
      </div>
      <TooltipGroup side="top" className="flex shrink-0 items-center gap-0.5">
        {holdable && (
          <RowAction
            label={`Hold ${job.track.title}`}
            tooltip="Hold track"
            onClick={() => void music.pause(job.id)}
          >
            <Pause className="size-3.5" />
          </RowAction>
        )}
        {resumable && (
          <RowAction
            label={job.state === "failed" ? `Retry ${job.track.title}` : `Resume ${job.track.title}`}
            tooltip={job.state === "failed" ? "Retry" : "Resume"}
            onClick={() => void music.resume(job.id)}
          >
            {job.state === "failed" ? <RotateCcw className="size-3.5" /> : <Play className="size-3.5" />}
          </RowAction>
        )}
        {job.state === "skipped" && (
          <RowAction
            label="Download again, replacing nothing (a numbered copy)"
            tooltip="Download copy"
            onClick={() => void music.resume(job.id, true)}
          >
            <DownloadGlyph className="size-3.5" />
          </RowAction>
        )}
        {job.path && hasEngine() && (
          <>
            <RowAction
              label={`Send ${job.track.title} to Discord`}
              tooltip="Send to Discord"
              onClick={() => {
                void engine
                  .discordSend([job.id])
                  .then(() =>
                    onAddToast?.({
                      id: `discord-send-${Date.now()}`,
                      type: "info",
                      title: "Queued for Discord",
                      message: `${trackLine(job.track)} queued for Discord delivery.`,
                    }),
                  )
                  .catch((e: unknown) =>
                    onAddToast?.({
                      id: `discord-err-${Date.now()}`,
                      type: "error",
                      title: "Could not send to Discord",
                      message: e instanceof Error ? e.message : String(e),
                    }),
                  );
              }}
            >
              <Discord className="size-3.5" />
            </RowAction>
            <RowAction
              label="Show in folder"
              tooltip="Show in folder"
              onClick={() => void engine.reveal(job.id)}
            >
              <FolderOpen className="size-3.5" />
            </RowAction>
          </>
        )}
        <RowAction
          label={deleteFilesOnRemove ? `Delete ${job.track.title} and local file` : `Remove ${job.track.title} from the list`}
          tooltip={deleteFilesOnRemove ? "Delete entry and file" : "Remove from list"}
          onClick={() => onRemove ? onRemove(job) : void music.remove(job.id, deleteFilesOnRemove)}
        >
          <Trash className="size-3.5" />
        </RowAction>
      </TooltipGroup>
      {job.state === "failed" && <div className="basis-full"><FailureActions item={musicJobToSnapshot(job)} onRetry={() => music.resume(job.id)} onRecover={onRecover} /></div>}
      <DownloadDisclosure item={musicJobToSnapshot(job)} onRemoveEntry={onRemove ? () => onRemove(job, false) : undefined} />
    </div>
  );
}

export function MusicView({
  music,
  prefill,
  onAddToast,
  onRemove,
  onRecover,
  deleteFilesOnRemove = false,
}: {
  music: ReturnType<typeof useMusic>;
  /** A link (and its captured session) the extension asked us to open here. */
  prefill?: { url: string; headers: [string, string][]; nonce: number; directory?: string } | null;
  onAddToast?: (toast: ToastItem) => void;
  onRemove?: (job: MusicJob, deleteFile?: boolean) => void;
  onRecover?: (item: QueueItem, kind: RecoveryKind) => Promise<void>;
  deleteFilesOnRemove?: boolean;
}) {
  const [input, setInput] = useState("");
  const [jobTab, setJobTab] = useState<"active" | "finished" | "failed">("active");
  const [queuedNote, setQueuedNote] = useState<string | null>(null);
  const [discordBusy, setDiscordBusy] = useState(false);
  const [installingFfmpeg, setInstallingFfmpeg] = useState(false);
  const [ffmpegInstallError, setFfmpegInstallError] = useState<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const handleInstallFfmpeg = async () => {
    setInstallingFfmpeg(true);
    setFfmpegInstallError(null);
    try {
      await music.installFfmpeg();
    } catch (e) {
      setFfmpegInstallError(e instanceof Error ? e.message : String(e));
    } finally {
      setInstallingFfmpeg(false);
    }
  };

  // Text fields commit on blur; the local copies keep typing responsive.
  const [template, setTemplate] = useState("");
  const [folder, setFolder] = useState("");
  const [customName, setCustomName] = useState("");
  const settings = music.settings;
  useEffect(() => {
    if (!settings) return;
    setTemplate(settings.output.filenameTemplate);
    setFolder(settings.output.directory ?? "");
    setCustomName(settings.output.folderName ?? "");
  }, [settings]);

  useEffect(() => {
    if (!prefill?.url) return;
    setInput(prefill.url);
    setQueuedNote(null);
    void music.analyze(prefill.url, prefill.headers.length ? prefill.headers : undefined);
    setTimeout(() => inputRef.current?.focus(), 50);
    // Keyed on the nonce so the same link sent twice re-fills.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prefill?.nonce]);

  const links = useMemo(() => linksIn(input), [input]);
  const many = links.length > 1;

  async function lookUp(e?: React.FormEvent) {
    e?.preventDefault();
    const trimmed = input.trim();
    if (!trimmed || music.analyzing) return;
    setQueuedNote(null);
    if (many) return;
    await music.analyze(links[0] ?? trimmed);
  }

  async function queueAll(autoDiscord = false) {
    if (music.analyzing) return;
    setQueuedNote(null);
    const result = await music.downloadMany(links, autoDiscord);
    const parts: string[] = [];
    if (result.queued > 0) parts.push(`${result.queued} queued`);
    if (result.skipped > 0) parts.push(`${result.skipped} already on disk`);
    setQueuedNote(parts.join(" · ") || null);
    if (result.failed.length > 0) {
      onAddToast?.({
        id: `music-batch-${Date.now()}`,
        type: "error",
        title: `${result.failed.length} of ${links.length} links could not be queued`,
        message: result.failed.join("\n"),
      });
    }
    setJobTab("active");
  }

  async function download(autoDiscord = false) {
    if (autoDiscord) setDiscordBusy(true);
    const enqueued = await music.download(autoDiscord, prefill?.directory);
    setDiscordBusy(false);
    if (!enqueued) return;
    const queued = enqueued.jobIds.length - enqueued.skipped;
    setQueuedNote(
      `${queued} queued${enqueued.skipped > 0 ? ` · ${enqueued.skipped} already on disk` : ""} → ${enqueued.directory}`,
    );
    setJobTab("active");
    if (autoDiscord) {
      let connected = true;
      try {
        connected = (await engine.discordStatus()).connected;
      } catch {}
      onAddToast?.(
        connected
          ? {
              id: `discord-auto-${Date.now()}`,
              type: "info",
              title: "Queued with Discord auto-delivery",
              message: "Each track is sent to Discord as it finishes.",
            }
          : {
              id: `discord-warn-${Date.now()}`,
              type: "error",
              title: "Discord not connected",
              message: "The tracks are queued, but Discord delivery is not set up. Connect it in the Discord tab.",
            },
      );
    }
  }

  const source = music.source;
  const selectedCount = music.selectedTracks.length;
  const allSelected = source ? selectedCount === source.tracks.length : false;
  const output = settings?.output;
  const bitrates = output ? BITRATES[output.format] : [];
  const lossyWarning =
    output && (output.format === "flac" || output.format === "alac" || output.format === "wav")
      ? "Every stream these services serve is lossy. A lossless container keeps it exactly, but cannot restore what was never there — expect a large file of source quality."
      : null;

  const groups = music.groups;
  const groupsFor = (tab: typeof jobTab) =>
    groups.filter((g) =>
      g.jobs.some((j) =>
        tab === "active"
          ? ACTIVE_STATES.includes(j.state)
          : tab === "finished"
            ? j.state === "finished" || j.state === "skipped"
            : j.state === "failed",
      ),
    );
  const TABS: { id: typeof jobTab; label: string; count: number }[] = [
    { id: "active", label: "Active", count: music.jobs.filter((j) => ACTIVE_STATES.includes(j.state)).length },
    { id: "finished", label: "Finished", count: music.jobs.filter((j) => j.state === "finished" || j.state === "skipped").length },
    { id: "failed", label: "Failed", count: music.jobs.filter((j) => j.state === "failed").length },
  ];
  const visibleGroups = groupsFor(jobTab);
  const visibleJobs = (g: MusicGroup) =>
    g.jobs.filter((j) =>
      jobTab === "active"
        ? ACTIVE_STATES.includes(j.state)
        : jobTab === "finished"
          ? j.state === "finished" || j.state === "skipped"
          : j.state === "failed",
    );

  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-7 px-6 py-6">
      {prefill?.directory && <p className="break-all text-[12px] text-ink">This download will use: {prefill.directory}</p>}
      <Section
        title="Find music"
        aside={input.length > 0 ? <ClearButton disabled={music.analyzing} onClick={() => {
          setInput("");
          music.clearSource();
          music.clearError();
          setQueuedNote(null);
          inputRef.current?.focus();
        }} /> : undefined}
        description="A track, album, playlist or artist link from Spotify, Apple Music, Deezer, YouTube, YouTube Music, SoundCloud or Bandcamp; a direct audio link; or a song name to search. Spotify, Apple Music and Deezer give the catalogue entry only — the recording is matched on YouTube Music and the row says which. Several links at once, one per line, are queued straight through."
      >
        <form autoComplete="off" onSubmit={(e) => void lookUp(e)} className="flex items-start gap-2">
          <div className="grid min-w-0 flex-1">
            <span aria-hidden="true" className="invisible col-start-1 row-start-1 min-w-0 whitespace-pre-wrap [overflow-wrap:anywhere] border border-transparent px-2.5 py-1.5 font-mono text-[12px] leading-relaxed">
              {`${input || MUSIC_INPUT_PLACEHOLDER} `}
            </span>
            <textarea
              ref={inputRef}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  if (many) void queueAll(false);
                  else void lookUp();
                }
              }}
              rows={1}
              placeholder={MUSIC_INPUT_PLACEHOLDER}
              aria-label="Music link or search" autoComplete="off"
              spellCheck={false}
              className="recess focus-ring col-start-1 row-start-1 h-full w-full min-w-0 resize-none overflow-hidden whitespace-pre-wrap [overflow-wrap:anywhere] rounded-hw px-2.5 py-1.5 font-mono text-[12px] leading-relaxed text-ink outline-none placeholder:text-ink-faint"
            />
          </div>
          {many ? (
            <Button type="button" onClick={() => void queueAll(false)} disabled={music.analyzing || !settings}>
              {music.analyzing ? "queuing…" : `queue all ${links.length}`}
            </Button>
          ) : (
            <Button type="submit" disabled={music.analyzing || !input.trim()}>
              {music.analyzing ? "looking up…" : linksIn(input).length === 1 ? "look up" : "search"}
            </Button>
          )}
        </form>

        {music.youtubeSession === false && (
          <div className="flex items-start gap-3 border border-caution/50 bg-caution/10 px-3 py-2.5">
            <Legend className="text-caution">no youtube session</Legend>
            <p className="flex-1 text-[12px] leading-relaxed text-ink">
              Spotify, Apple Music and Deezer hand over the catalogue entry only, so the recording is fetched from
              YouTube — and YouTube serves a downloader that is not signed in about twenty seconds of each track before
              refusing. Sign in to YouTube in a browser with the Zoltraak extension installed and play any video once; the
              extension shares the session and full-length downloads work. SoundCloud, Bandcamp and direct links need
              none of this.
            </p>
          </div>
        )}

        {music.tools && !music.tools.ffmpeg && (
          <div className="flex items-center justify-between gap-4 border border-caution/50 bg-caution/10 px-3.5 py-2.5">
            <div className="flex items-start gap-3 min-w-0 flex-1">
              <Legend className="text-caution shrink-0">ffmpeg missing</Legend>
              <p className="text-[12px] leading-relaxed text-ink">
                Converting, tagging and cover art need <span className="describer">ffmpeg</span>. Without it
                only “best available” works, and the file lands as it arrived, untagged.
              </p>
            </div>
            <button
              type="button"
              onClick={() => void handleInstallFfmpeg()}
              disabled={installingFfmpeg}
              className="focus-ring flex shrink-0 items-center gap-1.5 border border-caution bg-caution/20 px-3 py-1.5 text-[11px] legend text-ink hover:bg-caution/30 transition-colors disabled:opacity-50"
            >
              <DownloadGlyph className="size-3.5" />
              <span>{installingFfmpeg ? "Installing..." : "Install Automatically"}</span>
            </button>
          </div>
        )}
        {ffmpegInstallError && (
          <ErrorStrip message={ffmpegInstallError} onDismiss={() => setFfmpegInstallError(null)} />
        )}

        {music.error && <ErrorStrip message={music.error} onDismiss={music.clearError} />}
        {queuedNote && !source && <span className="describer text-[11px] text-clear">{queuedNote}</span>}
      </Section>

      {source && (
        <Section
          title="Found"
          aside={
            <Legend>
              {selectedCount} of {source.tracks.length} selected
              {source.total && source.total > source.tracks.length ? ` · ${source.total} reported` : ""}
            </Legend>
          }
        >
          <div className="border border-edge bg-face">
            <div className="flex items-center gap-3 border-b border-edge bg-recess px-4 py-2.5">
              <Cover url={source.coverUrl} size="size-11" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-[13px] text-ink">{source.title}</p>
                <p data-selectable className="describer mt-0.5 truncate text-[11px] text-ink-dim">
                  {SERVICE_LABEL[source.service]} {KIND_LABEL[source.kind]}
                  {source.owner ? ` · ${source.owner}` : ""} · {source.sourceUrl}
                </p>
              </div>
            </div>

            {source.note && (
              <p className="border-b border-edge px-4 py-2 text-[12px] leading-relaxed text-ink-dim">{source.note}</p>
            )}

            <div className="flex flex-wrap items-center gap-2 border-b border-edge px-4 py-2.5">
              <button
                type="button"
                onClick={allSelected ? music.selectNone : music.selectAll}
                className="focus-ring legend rounded-hw border border-edge-strong px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-face hover:text-ink"
              >
                {allSelected ? "select none" : "select all"}
              </button>
              <span className="flex-1" />
              {queuedNote && <span className="describer text-[11px] text-clear">{queuedNote}</span>}
              <Legend>
                {output ? `${FORMATS.find((f) => f.value === output.format)?.label ?? output.format}` : ""}
                {output && bitrates.length > 0 ? ` · ${output.bitrateKbps ?? bitrates[bitrates.length - 1]} kbps` : ""}
              </Legend>
              <Button onClick={() => void download(false)} disabled={selectedCount === 0 || !settings || discordBusy}>
                <span className="flex items-center gap-1.5">
                  <DownloadGlyph className="size-3.5" />
                  {allSelected ? `download all (${selectedCount})` : `download ${selectedCount}`}
                </span>
              </Button>
              <button
                type="button"
                onClick={() => void download(true)}
                disabled={selectedCount === 0 || !settings || discordBusy}
                title="Download and auto-send each track to Discord"
                aria-label="Download and auto-send to Discord"
                className="focus-ring flex h-[31px] w-[31px] shrink-0 items-center justify-center rounded-hw border border-edge-strong bg-face text-ink-dim transition-colors duration-150 hover:border-discord hover:bg-discord/10 hover:text-discord disabled:cursor-not-allowed disabled:opacity-45"
              >
                <Discord className="size-4" />
              </button>
            </div>

            <div role="group" aria-label="Tracks found" className="max-h-[28rem] overflow-y-auto">
              {source.tracks.map((track, index) => (
                <TrackRow
                  key={track.id}
                  track={track}
                  index={index}
                  selected={music.selected.has(track.id)}
                  onToggle={() => music.toggle(track.id)}
                />
              ))}
            </div>
          </div>
        </Section>
      )}

      <Section
        title="Output"
        description="What the files become. Every change here is saved as the default for the next download."
        aside={<SaveIndicator state={music.saveState} />}
      >
        {!output ? (
          <div className="flex flex-col items-center gap-3 border border-edge bg-face px-6 py-10 text-center">
            <span className="h-px w-24 bg-edge" />
            <Legend className="text-ink-dim">{hasEngine() ? "reading settings…" : "engine detached"}</Legend>
          </div>
        ) : (
          <div className="flex flex-col gap-5 border border-edge bg-face p-4">
            <div className="grid gap-4 md:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
              <Field label="Format" hint={lossyWarning ?? (output.format === "best" ? "Keeps the source stream as it is — AAC lands as .m4a, Opus as .opus, MP3 as .mp3 — with tags and cover written in. Nothing is re-encoded." : undefined)}>
                <SelectInput<AudioFormat>
                  value={output.format}
                  onChange={(format) => music.updateOutput({ format, bitrateKbps: null })}
                  options={FORMATS}
                  ariaLabel="Audio format"
                />
              </Field>
              <Field label="Bitrate" hint={bitrates.length === 0 ? "Not a choice for this format." : undefined}>
                <SelectInput<string>
                  value={String(output.bitrateKbps ?? "")}
                  onChange={(v) => music.updateOutput({ bitrateKbps: v ? Number(v) : null })}
                  options={[
                    { value: "", label: bitrates.length ? `default (${bitrates[bitrates.length - 1]} kbps)` : "—" },
                    ...bitrates.map((b) => ({ value: String(b), label: `${b} kbps` })),
                  ]}
                  ariaLabel="Bitrate"
                />
              </Field>
            </div>

            <div className="grid gap-4 md:grid-cols-2">
              <Field
                label="Music folder"
                hint={`Blank uses ${music.defaultDirectory ?? "a Music folder inside the download folder"}.`}
              >
                <TextInput
                  value={folder}
                  onChange={setFolder}
                  onBlur={() => music.updateOutput({ directory: folder.trim() || null })}
                  placeholder={music.defaultDirectory ?? ""}
                  mono
                  ariaLabel="Music folder"
                />
              </Field>
              <Field label="Playlists and albums go into">
                <SelectInput<FolderMode>
                  value={output.folderMode}
                  onChange={(folderMode) => music.updateOutput({ folderMode })}
                  options={FOLDER_MODES}
                  ariaLabel="Folder for a collection"
                />
                {output.folderMode === "custom" && (
                  <TextInput
                    value={customName}
                    onChange={setCustomName}
                    onBlur={() => music.updateOutput({ folderName: customName.trim() || null })}
                    placeholder="Folder name"
                    ariaLabel="Custom folder name"
                    className="mt-1.5"
                  />
                )}
              </Field>
            </div>

            <Field label="Filename" hint={`Placeholders: ${PLACEHOLDERS}. Extension is added by the format.`}>
              <TextInput
                value={template}
                onChange={setTemplate}
                onBlur={() => music.updateOutput({ filenameTemplate: template.trim() || "{artist} - {title}" })}
                mono
                ariaLabel="Filename template"
              />
            </Field>

            <div className="grid gap-x-8 md:grid-cols-2">
              <ToggleRow title="Number tracks in a collection" description="Prefix 01 -, 02 -… so an album stays in order in any file manager." checked={output.numberTracks} onChange={(numberTracks) => music.updateOutput({ numberTracks })} />
              <ToggleRow title="Write tags" description="Title, artist, album, track number, year, genre and the source link, into the file." checked={output.embedMetadata} onChange={(embedMetadata) => music.updateOutput({ embedMetadata })} />
              <ToggleRow title="Embed cover art" description="Written into the file, so players and Explorer show it. MP3, M4A and FLAC carry a picture; Opus, Ogg and WAV cannot, and are left alone." checked={output.embedCover} onChange={(embedCover) => music.updateOutput({ embedCover })} />
              <ToggleRow title="Look up missing cover art" description="A track matched on YouTube arrives with a 16:9 video still. When there is no real cover, one is looked up on Apple Music and Deezer, and only used if it is confidently the same release." checked={output.fetchArtwork} onChange={(fetchArtwork) => music.updateOutput({ fetchArtwork })} />
              <ToggleRow title="Save the cover beside the files" description="cover.jpg and folder.jpg in a playlist or album folder, using the collection's own cover. A single track gets its picture next to it." checked={output.saveCoverFile} onChange={(saveCoverFile) => music.updateOutput({ saveCoverFile })} />
              <ToggleRow title="Fetch lyrics" description="From lrclib.net, embedded in the file; a synced .lrc beside it when one exists." checked={output.fetchLyrics} onChange={(fetchLyrics) => music.updateOutput({ fetchLyrics })} />
              <ToggleRow title="Write an .m3u8 for a collection" description="A playlist file listing the tracks in order, for any player." checked={output.writePlaylistFile} onChange={(writePlaylistFile) => music.updateOutput({ writePlaylistFile })} />
              <ToggleRow title="Skip tracks already on disk" description="A track whose file exists is marked on disk rather than fetched again." checked={output.skipExisting} onChange={(skipExisting) => music.updateOutput({ skipExisting })} />
              <ToggleRow title="Normalise loudness" description="EBU R128 to −14 LUFS, the level most players aim for. Forces a re-encode." checked={output.normalizeLoudness} onChange={(normalizeLoudness) => music.updateOutput({ normalizeLoudness })} />
            </div>

            <div className="grid gap-4 border-t border-edge pt-4 md:grid-cols-2">
              <Field label="Tracks at once" hint="Lookups, downloads and conversions running in parallel.">
                <SelectInput<string>
                  value={String(settings.maxConcurrent)}
                  onChange={(v) => music.updateSettings({ maxConcurrent: Number(v) })}
                  options={[1, 2, 3, 4, 6, 8].map((n) => ({ value: String(n), label: String(n) }))}
                  ariaLabel="Concurrent tracks"
                />
              </Field>
              <ToggleRow
                title="Match on YouTube Music first"
                description="Its song search returns the recording itself; plain YouTube is the fallback either way."
                checked={settings.preferYoutubeMusic}
                onChange={(preferYoutubeMusic) => music.updateSettings({ preferYoutubeMusic })}
              />
            </div>
          </div>
        )}
      </Section>

      <Section
        title="Tracks"
        aside={
          <div className="flex flex-wrap items-center gap-2">
            {music.jobs.some((j) => j.state === "finished" || j.state === "skipped") && (
              <button
                type="button"
                onClick={() => void music.clearDone()}
                className="focus-ring legend rounded-hw border border-edge-strong px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-face hover:text-ink"
              >
                clear finished
              </button>
            )}
            {hasEngine() && (
              <TooltipGroup side="bottom">
                <button
                  type="button"
                  onClick={() => void engine.revealFolder(music.defaultDirectory ?? undefined)}
                  data-tooltip="Open music folder (Ctrl + E)"
                  className="t-tt-trigger focus-ring legend flex items-center gap-1.5 rounded-hw border border-edge-strong px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-face hover:text-ink"
                >
                  <FolderOpen className="size-3.5" />
                  Open folder
                </button>
              </TooltipGroup>
            )}
            <SlideGroup role="tablist" aria-label="Music job state" onKeyDown={onTabKeyDown} className="flex overflow-hidden rounded-hw border border-edge-strong">
              {TABS.map((tab) => (
                <button
                  key={tab.id}
                  type="button"
                  role="tab"
                  aria-selected={jobTab === tab.id}
                  tabIndex={jobTab === tab.id ? 0 : -1}
                  onClick={() => setJobTab(tab.id)}
                  className={`focus-ring legend border-r border-edge px-2.5 py-1 text-[11px] transition-colors last:border-r-0 ${
                    jobTab === tab.id ? "text-on-signal" : "text-ink-dim hover:bg-face hover:text-ink"
                  }`}
                >
                  {tab.label} {tab.count}
                </button>
              ))}
            </SlideGroup>
          </div>
        }
      >
        {visibleGroups.length === 0 ? (
          <div className="flex flex-col items-center gap-3 border border-edge bg-face px-6 py-12 text-center">
            <span className="h-px w-24 bg-edge" />
            <Legend className="text-ink-dim">
              {jobTab === "finished" ? "nothing finished yet" : jobTab === "failed" ? "no faults" : "no tracks queued"}
            </Legend>
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            {visibleGroups.map((group) => (
              <div key={group.key} className="border border-edge bg-face">
                {group.collectionId && <GroupHeader group={group} music={music} onAddToast={onAddToast} />}
                {visibleJobs(group).map((job) => (
                  <JobRow key={job.id} job={job} inCollection={Boolean(group.collectionId)} music={music} onAddToast={onAddToast} onRemove={onRemove} onRecover={onRecover} deleteFilesOnRemove={deleteFilesOnRemove} />
                ))}
              </div>
            ))}
          </div>
        )}
      </Section>
    </div>
  );
}

/** A named behaviour with its lever, as the settings screens draw one. */
function ToggleRow({
  title,
  description,
  checked,
  onChange,
}: {
  title: string;
  description: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <div className="flex items-start justify-between gap-6 border-b border-edge py-3 last:border-b-0 md:[&:nth-last-child(2)]:border-b-0">
      <div className="min-w-0">
        <p className="text-[13px] text-ink">{title}</p>
        <p className="mt-0.5 max-w-[48ch] text-[12px] leading-relaxed text-ink-dim">{description}</p>
      </div>
      <Switch label={title} checked={checked} onChange={onChange} />
    </div>
  );
}
