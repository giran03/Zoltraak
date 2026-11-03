import type { DownloadSnapshot, MusicJob, VideoJob, VideoValidation } from "./engine";
import type { DownloadState } from "./types";

/** A queue row that also remembers which pipeline it came from. */
export type QueueItem = DownloadSnapshot & {
  isVideo?: boolean;
  isMusic?: boolean;
  thumbnail?: string | null;
  /**
   * The file on disk, once there is one.
   *
   * The three pipelines each store this somewhere different — the queue keeps
   * a directory and a filename, video and music keep a whole path — so the row
   * carries the resolved answer rather than making every consumer re-derive it
   * and get it wrong for two pipelines out of three.
   */
  path?: string | null;
  quality?: string | null;
  stage?: string | null;
  validation?: VideoValidation | null;
};

/** `directory` and `filename` as one path, in the separator already in use. */
function joinPath(directory: string, filename: string): string {
  if (!directory) return filename;
  const separator = directory.includes("\\") ? "\\" : "/";
  return directory.endsWith(separator)
    ? `${directory}${filename}`
    : `${directory}${separator}${filename}`;
}

/** Extract directory portion from a path. */
function directoryOf(path?: string | null): string {
  if (!path) return "";
  const lastSlash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return lastSlash > -1 ? path.slice(0, lastSlash) : "";
}

/**
 * Present a video job as a queue row so both pipelines can share one list and
 * one row component.
 *
 * This was previously duplicated verbatim in `App` and `HistoryView`, which
 * meant a fix to one never reached the other.
 *
 * Rate, ETA and total all come straight from the engine now, so a video row
 * carries the same live readings as a queued download rather than a static bar.
 * For a segmented source the total is an estimate extrapolated from the
 * segments already fetched — approximate early, and it tightens as it goes.
 */
export function videoJobToSnapshot(job: VideoJob): QueueItem {
  const isFinished = job.state === "finished";
  const state: DownloadState =
    isFinished ? "finished"
    : job.state === "failed" ? "failed"
    : job.state === "paused" ? "paused"
    : "downloading";

  return {
    id: job.id,
    filename: job.title ?? "Video download",
    url: job.webpageUrl,
    directory: directoryOf(job.path),
    state,
    downloadedBytes: job.downloadedBytes,
    totalBytes: isFinished ? job.downloadedBytes : (job.totalBytes ?? null),
    connections: 1,
    bytesPerSecond: job.bytesPerSecond,
    etaSeconds: job.etaSeconds,
    priority: 0,
    error: job.error,
    createdAt: job.createdAt,
    isVideo: true,
    thumbnail: job.thumbnail,
    path: job.path,
    quality: job.formatId,
    validation: job.validation,
  };
}

/**
 * Present a music job as a queue row.
 *
 * A music job's own states are finer than the register's (resolving,
 * converting, skipped); they fold into the nearest register aspect, and the
 * Music view is where the detail lives. A skipped track counts as finished —
 * the file is there, which is what "finished" means to the register.
 */
export function musicJobToSnapshot(job: MusicJob): QueueItem {
  const state: DownloadState =
    job.state === "finished" || job.state === "skipped" ? "finished"
    : job.state === "failed" ? "failed"
    : job.state === "paused" ? "paused"
    : job.state === "queued" ? "waiting"
    : "downloading";
  const done = state === "finished";
  const artist = job.track.artists.join(", ");
  const filename = job.path
    ? job.path.split(/[\\/]/).pop() || job.track.title
    : artist ? `${artist} - ${job.track.title}` : job.track.title;

  return {
    id: job.id,
    filename,
    url: job.track.sourceUrl,
    directory: job.directory,
    state,
    downloadedBytes: job.downloadedBytes,
    totalBytes: done ? (job.totalBytes ?? job.downloadedBytes) : job.totalBytes,
    connections: 1,
    bytesPerSecond: job.bytesPerSecond,
    etaSeconds: job.etaSeconds,
    priority: 0,
    error: job.error,
    createdAt: job.createdAt,
    isMusic: true,
    thumbnail: job.track.coverUrl,
    quality: job.format,
    stage: job.stage,
    path: job.path,
  };
}

/**
 * Queue rows, video jobs and music jobs as one list, newest first. Queue rows
 * win on an id collision — the queue is the authoritative store for anything
 * it knows about.
 */
export function mergeItems(
  items: DownloadSnapshot[],
  videoJobs: VideoJob[],
  musicJobs: MusicJob[] = [],
): QueueItem[] {
  const map = new Map<string, QueueItem>();
  for (const item of items) {
    map.set(item.id, { ...item, path: joinPath(item.directory, item.filename) });
  }
  for (const job of videoJobs) {
    const snapshot = videoJobToSnapshot(job);
    if (!map.has(snapshot.id)) map.set(snapshot.id, snapshot);
  }
  for (const job of musicJobs) {
    const snapshot = musicJobToSnapshot(job);
    if (!map.has(snapshot.id)) map.set(snapshot.id, snapshot);
  }
  return Array.from(map.values()).sort(
    (a, b) =>
      (b.createdAt ? new Date(b.createdAt).getTime() : 0) -
      (a.createdAt ? new Date(a.createdAt).getTime() : 0),
  );
}
