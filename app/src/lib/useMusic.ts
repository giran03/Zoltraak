import { reconcileRows } from "./polling";
import { usePolling } from "./usePolling";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  engine,
  hasEngine,
  type MusicEnqueued,
  type MusicJob,
  type MusicSettings,
  type MusicSource,
  type MusicTools,
  type MusicTrack,
  type OutputOptions,
} from "./engine";

/** The links in a pasted blob, one per line or separated by whitespace. */
export function linksIn(input: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const token of input.split(/\s+/)) {
    const t = token.trim();
    if (!t) continue;
    if (!(t.startsWith("http://") || t.startsWith("https://") || t.startsWith("spotify:"))) continue;
    if (seen.has(t)) continue;
    seen.add(t);
    out.push(t);
  }
  return out;
}

/**
 * The engine's marker on a track that failed only for want of a signed-in
 * YouTube session. Mirrors `WAITING_FOR_YOUTUBE_SESSION` in the engine, which
 * both writes it and watches for the sign-in that clears it.
 */
export const WAITING_FOR_YOUTUBE_SESSION = "waiting for a YouTube sign-in";

/**
 * Whether this track is not broken but waiting: the engine requeues it by
 * itself the moment the extension shares a YouTube sign-in, so presenting it
 * as a fault would ask the user to fix something that is already in hand.
 */
export function isWaitingForSession(job: MusicJob): boolean {
  return job.state === "failed" && job.stage === WAITING_FOR_YOUTUBE_SESSION;
}

/** A track's line, the way a list shows it: `Artist — Title`. */
export function trackLine(track: MusicTrack): string {
  const artist = track.artists.join(", ");
  return artist ? `${artist} — ${track.title}` : track.title;
}

/** `3:45`. */
export function formatClock(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds <= 0) return "—";
  const total = Math.round(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
    : `${m}:${String(s).padStart(2, "0")}`;
}

/** Jobs that share a collection, in playlist order — or one lone single. */
export type MusicGroup = {
  /** The collection id, or the job id for a single. */
  key: string;
  collectionId: string | null;
  title: string;
  url: string | null;
  jobs: MusicJob[];
  createdAt: string;
};

export function groupJobs(jobs: MusicJob[]): MusicGroup[] {
  const groups = new Map<string, MusicGroup>();
  for (const job of jobs) {
    const key = job.collectionId ?? job.id;
    const existing = groups.get(key);
    if (existing) {
      existing.jobs.push(job);
      if (job.createdAt < existing.createdAt) existing.createdAt = job.createdAt;
    } else {
      groups.set(key, {
        key,
        collectionId: job.collectionId,
        title: job.collectionTitle ?? trackLine(job.track),
        url: job.collectionUrl ?? job.track.sourceUrl,
        jobs: [job],
        createdAt: job.createdAt,
      });
    }
  }
  const out = Array.from(groups.values());
  for (const group of out) group.jobs.sort((a, b) => a.position - b.position);
  // Newest paste first, the same order the other job lists use.
  out.sort((a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime());
  return out;
}

export type BatchResult = { queued: number; skipped: number; failed: string[] };

/**
 * Paste-a-link music capture: resolve a link (or a search) into tracks, hand
 * the kept ones to the music manager with the output options, and follow the
 * resulting jobs.
 *
 * The output options double as the music settings: whatever the Output panel
 * shows is what the next download uses, and it is saved as the default as it
 * is edited — there is no separate "save as default" ritual to forget.
 */
export function useMusic(active = true) {
  const [jobs, setJobs] = useState<MusicJob[]>([]);
  const [source, setSource] = useState<MusicSource | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [settings, setSettings] = useState<MusicSettings | null>(null);
  const [tools, setTools] = useState<MusicTools | null>(null);
  const [defaultDirectory, setDefaultDirectory] = useState<string | null>(null);
  // Whether the extension has shared a signed-in YouTube session. Without one
  // YouTube's CDN serves an anonymous downloader only a preview, so every
  // track matched from Spotify, Apple Music or Deezer fails part-way — worth
  // saying before a playlist is queued rather than once per track after.
  const [youtubeSession, setYoutubeSession] = useState<boolean | null>(null);
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const headersRef = useRef<{ url: string; headers: [string, string][] } | null>(null);

  const refreshJobs = useCallback(async () => {
    if (!hasEngine()) return;
    try {
      const next = await engine.musicJobs();
      setJobs((previous) => reconcileRows(previous, next));
    } catch {
      // A missed poll tick is not worth a toast.
    }
  }, []);

  // Same adaptive poll as the video view: fast while something moves, slow
  // while idle, slower still when the window is hidden.
  const activeRef = useRef(false);
  useEffect(() => {
    activeRef.current = jobs.some((j) =>
      ["queued", "resolving", "downloading", "converting"].includes(j.state),
    );
  }, [jobs]);

  usePolling(refreshJobs, () => activeRef.current ? 400 : 2000);

  // Load Music-only setup on first visit; jobs remain available to Downloads.
  const [initialized, setInitialized] = useState(active);
  if (active && !initialized) setInitialized(true);
  useEffect(() => {
    if (!initialized || !hasEngine()) return;
    let cancelled = false;
    void engine
      .musicSettings()
      .then((s) => {
        if (!cancelled) setSettings(s);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      });
    void engine
      .musicTools()
      .then((t) => {
        if (!cancelled) setTools(t);
      })
      .catch(() => {});
    void engine
      .musicDirectory()
      .then((d) => {
        if (!cancelled) setDefaultDirectory(d);
      })
      .catch(() => {});
    void engine
      .sessions()
      .then((sessions) => {
        if (cancelled) return;
        setYoutubeSession(sessions.some((s) => s.domain === "youtube.com" && s.signedIn));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [initialized]);

  const resetTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const saveSettings = useCallback(
    async (next: MusicSettings) => {
      if (!hasEngine()) {
        setSettings(next);
        return true;
      }
      clearTimeout(resetTimer.current);
      setSaveState("saving");
      try {
        const saved = await engine.updateMusicSettings(next);
        setSettings(saved);
        setSaveState("saved");
        resetTimer.current = setTimeout(() => setSaveState("idle"), 2000);
        return true;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setSaveState("error");
        return false;
      }
    },
    [],
  );

  /** Change one or more output options; they are saved as the default. */
  const updateOutput = useCallback(
    (patch: Partial<OutputOptions>) => {
      if (!settings) return;
      const unchanged = (Object.keys(patch) as (keyof OutputOptions)[]).every(
        (key) => JSON.stringify(settings.output[key]) === JSON.stringify(patch[key]),
      );
      if (unchanged) return;
      void saveSettings({ ...settings, output: { ...settings.output, ...patch } });
    },
    [settings, saveSettings],
  );

  const updateSettings = useCallback(
    (patch: Partial<Omit<MusicSettings, "output">>) => {
      if (!settings) return;
      void saveSettings({ ...settings, ...patch });
    },
    [settings, saveSettings],
  );

  const analyze = useCallback(async (input: string, headers?: [string, string][]) => {
    setError(null);
    setSource(null);
    setSelected(new Set());
    headersRef.current = headers?.length ? { url: input, headers } : null;
    setAnalyzing(true);
    try {
      const found = await engine.analyzeMusic(input, headers);
      setSource(found);
      // Everything starts selected; a track the service itself calls
      // unavailable starts unselected, since it is the one most likely to fail.
      setSelected(new Set(found.tracks.filter((t) => !t.unavailable).map((t) => t.id)));
      return found;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return null;
    } finally {
      setAnalyzing(false);
    }
  }, []);

  const selectedTracks = useMemo(
    () => (source ? source.tracks.filter((t) => selected.has(t.id)) : []),
    [source, selected],
  );

  const toggle = useCallback((id: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  }, []);

  const selectAll = useCallback(() => {
    setSelected(new Set(source?.tracks.map((t) => t.id) ?? []));
  }, [source]);

  const selectNone = useCallback(() => setSelected(new Set()), []);

  /** Queue the kept tracks of the current source with the current options. */
  const download = useCallback(
    async (autoDiscord = false, directory?: string): Promise<MusicEnqueued | null> => {
      if (!source || !settings || selectedTracks.length === 0 || !hasEngine()) return null;
      setError(null);
      try {
        const enqueued = await engine.startMusicDownload(
          { ...source, tracks: selectedTracks },
          directory ? { ...settings.output, directory, folderMode: "none" } : settings.output,
          autoDiscord,
        );
        await refreshJobs();
        return enqueued;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return null;
      }
    },
    [source, settings, selectedTracks, refreshJobs],
  );

  /**
   * Several links at once: each is resolved and queued whole with the current
   * options, one after another. A link that fails is reported and the rest
   * still go through — one dead playlist should not cost the other five.
   */
  const downloadMany = useCallback(
    async (links: string[], autoDiscord = false): Promise<BatchResult> => {
      const result: BatchResult = { queued: 0, skipped: 0, failed: [] };
      if (!settings || !hasEngine()) return result;
      setError(null);
      setAnalyzing(true);
      try {
        for (const link of links) {
          try {
            const found = await engine.analyzeMusic(link);
            const tracks = found.tracks.filter((t) => !t.unavailable);
            if (tracks.length === 0) {
              result.failed.push(`${link}: nothing downloadable`);
              continue;
            }
            const enqueued = await engine.startMusicDownload(
              { ...found, tracks },
              settings.output,
              autoDiscord,
            );
            result.queued += enqueued.jobIds.length - enqueued.skipped;
            result.skipped += enqueued.skipped;
          } catch (e) {
            result.failed.push(`${link}: ${e instanceof Error ? e.message : String(e)}`);
          }
        }
      } finally {
        setAnalyzing(false);
        await refreshJobs();
      }
      return result;
    },
    [settings, refreshJobs],
  );

  const act = useCallback(
    async (f: () => Promise<unknown>) => {
      try {
        await f();
        await refreshJobs();
        return true;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return false;
      }
    },
    [refreshJobs],
  );

  const groups = useMemo(() => groupJobs(jobs), [jobs]);
  const pause = useCallback((id: string) => act(() => engine.pauseMusicJob(id)), [act]);
  const resume = useCallback((id: string, force?: boolean) => act(() => engine.resumeMusicJob(id, force)), [act]);
  const remove = useCallback((id: string, deleteFile = false) => act(() => engine.removeMusicJob(id, deleteFile)), [act]);

  return {
    jobs,
    groups,
    source,
    analyzing,
    error,
    selected,
    selectedTracks,
    settings,
    tools,
    defaultDirectory,
    youtubeSession,
    saveState,
    clearError: () => setError(null),
    clearSource: () => {
      setSource(null);
      setSelected(new Set());
    },
    analyze,
    toggle,
    selectAll,
    selectNone,
    download,
    downloadMany,
    updateOutput,
    updateSettings,
    pause,
    resume,
    remove,
    removeCollection: (collectionId: string) => act(() => engine.removeMusicCollection(collectionId)),
    retryCollection: (collectionId: string) => act(() => engine.retryMusicCollection(collectionId)),
    clearDone: () => act(() => engine.clearMusicDone()),
    refreshJobs,
    reloadTools: () => {
      void engine.musicTools().then(setTools).catch(() => {});
    },
    installFfmpeg: async () => {
      const t = await engine.installFfmpeg();
      setTools(t);
      return t;
    },
  };
}
