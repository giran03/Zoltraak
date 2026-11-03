import { reconcileRows } from "./polling";
import { usePolling } from "./usePolling";
import { choosePreset } from "./videoPresets";
import { useCallback, useEffect, useRef, useState } from "react";
import { type VideoFormat, type VideoJob, type VideoMetadata, type VideoDownloadOptions, engine, hasEngine } from "./engine";

/** Whether picking this format yields a file with sound. Mirrors the engine's
 * `VideoFormat::plays_with_audio`: a separate audio stream counts, and an
 * unknown `hasAudio` is assumed fine — most progressive files do have audio. */
export function playsWithAudio(format: VideoFormat): boolean {
  return format.requiresMerge || format.hasAudio !== false;
}

/**
 * Best-first, the same order the engine uses: sound before resolution.
 *
 * `requiresMerge` is no longer a ranking key. It used to come first, which put
 * a merge pair above a complete muxed file of the same height for no benefit —
 * and, worse, put a video-only track that *lacked* a pair above everything.
 */
export function sortFormats(formats: VideoFormat[]): VideoFormat[] {
  return [...formats].sort((a, b) => {
    const audioRank = (f: VideoFormat) => f.requiresMerge || f.hasAudio === true ? 2 : f.hasAudio == null ? 1 : 0;
    const pA = audioRank(a);
    const pB = audioRank(b);
    if (pA !== pB) {
      return pB - pA;
    }
    const hA = a.height ?? 0;
    const hB = b.height ?? 0;
    if (hA !== hB) {
      return hB - hA;
    }
    const brA = a.bitrate ?? 0;
    const brB = b.bitrate ?? 0;
    if (brA !== brB) {
      return brB - brA;
    }
    const fsA = a.filesize ?? 0;
    const fsB = b.filesize ?? 0;
    return fsB - fsA;
  });
}

export function extractWebsiteName(url: string): string {
  try {
    const parsed = new URL(url);
    let host = parsed.hostname.toLowerCase();
    if (host.includes("youtube.com") || host.includes("youtu.be")) return "youtube";
    if (host.includes("facebook.com") || host.includes("fb.watch")) return "facebook";
    if (host.includes("tiktok.com")) return "tiktok";
    if (host.includes("instagram.com")) return "instagram";
    if (host.includes("twitter.com")) return "twitter";
    if (host === "x.com" || host.endsWith(".x.com")) return "x";
    if (host.includes("reddit.com") || host.includes("v.redd.it")) return "reddit";
    if (host.includes("vimeo.com")) return "vimeo";

    for (const prefix of ["www.", "m.", "web.", "mobile.", "v.", "player.", "embed.", "app.", "cdn."]) {
      if (host.startsWith(prefix)) host = host.slice(prefix.length);
    }
    const parts = host.split(".");
    if (parts.length >= 2) {
      const p2 = parts[parts.length - 2];
      const p3 = parts[parts.length - 3];
      if (parts.length >= 3 && p2 && ["co", "com", "org", "net", "gov", "edu"].includes(p2) && p3) {
        return p3;
      }
      if (p2) return p2;
    }
    return parts[0] || "website";
  } catch {
    return "website";
  }
}

export function isMissingOrSiteDefaultTitle(title: string | undefined, url: string): boolean {
  if (!title || !title.trim()) return true;
  const trimmed = title.trim();
  const lower = trimmed.toLowerCase();
  const genericList = [
    "video", "video.mp4", "watch", "play", "embed", "stream", "media", "index", "index.html",
    "untitled", "download", "zoltraak", "tiktok", "tiktok - make your day", "tiktok video",
    "instagram", "instagram video", "facebook", "facebook video", "facebook - log in or sign up",
    "youtube", "youtube video", "twitter", "twitter video", "x", "x video", "reddit", "reddit video",
    "vimeo", "vimeo video"
  ];
  if (genericList.includes(lower)) return true;
  const siteName = extractWebsiteName(url);
  if (lower === siteName || lower === `${siteName} video` || lower === `watch ${siteName}`) return true;
  if (lower.startsWith("tiktok - trends start here") || lower.startsWith("tiktok - make your day")) return true;
  return false;
}

export function ensureValidTitle(title: string | undefined, url: string): string {
  const siteName = extractWebsiteName(url);
  if (isMissingOrSiteDefaultTitle(title, url)) {
    const chars = "abcdefghijklmnopqrstuvwxyz0123456789";
    let randStr = "";
    for (let i = 0; i < 6; i++) {
      randStr += chars.charAt(Math.floor(Math.random() * chars.length));
    }
    return `${siteName}_${randStr}`;
  }
  return title!.trim();
}

/**
 * Paste-a-link video capture: analyze a URL into a set of formats, start a
 * job for the one the user picks, and track jobs to completion.
 *
 * Jobs have no push channel of their own (unlike the download queue) — a
 * short poll is simple and cheap for a feature that rarely runs more than a
 * couple of jobs at once.
 */
export function useVideo() {
  const [jobs, setJobs] = useState<VideoJob[]>([]);
  const [metadata, setMetadata] = useState<VideoMetadata | null>(null);
  // Mirrors `metadata` synchronously so `analyze` can read the current value
  // (for the preserve-existing merge) and return the freshly computed result
  // without waiting for a re-render — which is what lets a caller auto-download
  // the best format immediately after analyzing.
  const metadataRef = useRef<VideoMetadata | null>(null);
  const analysisGeneration = useRef(0);
  const [analyzing, setAnalyzing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The headers `analyze` resolved metadata with — reused by `download` for
  // this same metadata, since a login-gated page needs the same session to
  // fetch the actual video/audio segments that it needed to see formats at all.
  const [headers, setHeaders] = useState<[string, string][] | undefined>(undefined);

  const refreshJobs = useCallback(async () => {
    if (!hasEngine()) return;
    try {
      const next = await engine.videoJobs();
      setJobs((previous) => reconcileRows(previous, next));
    } catch {
      // A missed poll tick is not worth surfacing as an error toast.
    }
  }, []);

  // Track whether there are active jobs in a ref so the polling timer can
  // adapt its interval without being re-registered on every state update.
  const hasActiveJobsRef = useRef(false);
  useEffect(() => {
    hasActiveJobsRef.current = jobs.some((j) => j.state === "downloading" || j.state === "merging");
  }, [jobs]);

  // A single stable polling loop — never destroyed and re-created while jobs
  // are active. Adapts its interval via the ref above, and respects the Page
  // Visibility API so it throttles to a slow tick when the window is hidden.
  usePolling(refreshJobs, () => hasActiveJobsRef.current ? 350 : 2000);


  const analyze = useCallback(
    async (
      url: string,
      requestHeaders?: [string, string][],
      options?: { preserveExisting?: boolean; titleOverride?: string; useFallback?: boolean },
    ) => {
      const generation = ++analysisGeneration.current;
      setError(null);
      if (!options?.preserveExisting) {
        metadataRef.current = null;
        setMetadata(null);
      }
      setHeaders(requestHeaders);
      setAnalyzing(true);
      try {
        const res = await engine.analyzeVideo(url, requestHeaders, options?.useFallback);
        if (generation !== analysisGeneration.current) return null;
        const prev = metadataRef.current;
        let next: VideoMetadata | null;
        const previousId = prev?.formats.find(f => f.validation?.contentId)?.validation?.contentId;
        const currentId = res?.formats.find(f => f.validation?.contentId)?.validation?.contentId;
        const sameItem = previousId && currentId ? previousId === currentId : prev?.webpageUrl === res?.webpageUrl;
        if (options?.preserveExisting && prev && res && sameItem) {
          const mergedFormats = [...prev.formats, ...res.formats];
          const seen = new Set<string>();
          const deduped = mergedFormats.filter((f) => {
            const u = f.videoUrl || f.id;
            if (seen.has(u)) return false;
            seen.add(u);
            return true;
          });
          next = {
            ...res,
            title: prev.title || res.title,
            thumbnail: prev.thumbnail || res.thumbnail,
            formats: sortFormats(deduped),
          };
        } else {
          next = res ? { ...res, formats: sortFormats(res.formats) } : res;
        }
        // A raw CDN link (format id "direct") carries no real title of its
        // own — the engine falls back to the URL's filename, a hash the user
        // never sees on the source. The caller's `titleOverride` (the source
        // page's document.title, from the extension) stands in for it there;
        // every other extractor already returns a genuine title/caption, so
        // this never overrides those.
        if (next && options?.titleOverride && next.formats.length > 0 && next.formats.every((f) => f.id === "direct")) {
          next = { ...next, title: ensureValidTitle(options.titleOverride, url) };
        } else if (next) {
          next = { ...next, title: ensureValidTitle(next.title, url) };
        }
        metadataRef.current = next ?? null;
        setMetadata(next);
        return next ?? null;
      } catch (e) {
        if (generation !== analysisGeneration.current) return null;
        if (!options?.preserveExisting) {
          setError(e instanceof Error ? e.message : String(e));
        }
        return null;
      } finally {
        if (generation === analysisGeneration.current) setAnalyzing(false);
      }
    },
    [],
  );

  const download = useCallback(
    async (format: VideoFormat, autoDiscord?: boolean, options?: VideoDownloadOptions, directory?: string) => {
      if (!metadata) return false;
      setError(null);
      try {
        await engine.startVideoDownload(metadata.title, metadata.webpageUrl, { ...format, downloadOptions: { ...format.downloadOptions, thumbnail: format.downloadOptions?.thumbnail ?? metadata.thumbnail } }, directory, headers, autoDiscord, options);
        await refreshJobs();
        return true;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return false;
      }
    },
    [metadata, headers, refreshJobs],
  );

  // Start the single best (top-ranked) format of an explicit metadata object,
  // rather than the one in state — used by the extension's "Download this
  // video" one-click flow, which analyzes and downloads without a format pick.
  // Taking `meta`/`requestHeaders` as arguments sidesteps the state/closure lag
  // right after `analyze` resolves.
  const downloadBest = useCallback(
    async (meta: VideoMetadata, requestHeaders?: [string, string][]) => {
      const format = choosePreset(meta.formats, "best");
      if (!format) {
        setError(meta.formats.length ? "This post contains multiple videos. Choose a clip below." : "No video format is available.");
        return false;
      }
      setError(null);
      try {
        await engine.startVideoDownload(meta.title, meta.webpageUrl, { ...format, downloadOptions: { ...format.downloadOptions, thumbnail: format.downloadOptions?.thumbnail ?? meta.thumbnail } }, undefined, requestHeaders);
        await refreshJobs();
        return true;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return false;
      }
    },
    [refreshJobs],
  );

  const pause = useCallback(
    async (id: string) => {
      try {
        await engine.pauseVideoJob(id);
        await refreshJobs();
        return true;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return false;
      }
    },
    [refreshJobs],
  );

  const resume = useCallback(
    async (id: string) => {
      setError(null);
      try {
        await engine.resumeVideoJob(id);
        await refreshJobs();
        return true;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return false;
      }
    },
    [refreshJobs],
  );

  const remove = useCallback(
    async (id: string, deleteFile = false) => {
      try {
        await engine.removeVideoJob(id, deleteFile);
        await refreshJobs();
        return true;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return false;
      }
    },
    [refreshJobs],
  );

  return {
    jobs,
    metadata,
    analyzing,
    error,
    clearError: () => setError(null),
    clearMetadata: () => {
      ++analysisGeneration.current;
      setAnalyzing(false);
      metadataRef.current = null;
      setMetadata(null);
    },
    analyze,
    download,
    downloadBest,
    pause,
    resume,
    remove,
  };
}
