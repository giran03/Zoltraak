import { useCallback, useMemo, useRef, useState } from "react";
import { type ImageAlbum, type ImageItem, engine, hasEngine } from "./engine";
import { extractWebsiteName } from "./useVideo";

/**
 * A filename for one item of an album.
 *
 * Photo CDNs name their objects by content hash, so saving a gallery straight
 * from its URLs produces a folder of `GKx7_2mWQAA1nP4.jpg` with nothing to say
 * which post any of them came from. The post's own title is the useful name,
 * and the index is what keeps a carousel in the order it was posted in.
 */
export function itemFilename(album: ImageAlbum, item: ImageItem, index: number): string {
  const site = extractWebsiteName(album.sourceUrl);
  const stem = slug(album.title) || site;
  // A single-item post needs no counter; a gallery is unreadable without one,
  // and a zero-padded index keeps it in order in the file manager too.
  const total = album.items.length;
  const counter = total > 1 ? `_${String(index + 1).padStart(String(total).length, "0")}` : "";
  return `${stem}${counter}.${item.extension}`;
}

/**
 * The name without its container suffix.
 *
 * A video job builds its own filename from the title and appends the container
 * it ends up muxing to — which is not necessarily the one the extractor
 * advertised. Passing `clip_1.mp4` as the title would produce `clip_1.mp4.mp4`.
 */
function stripExtension(name: string): string {
  return name.replace(/\.[^.]+$/, "");
}

/** A title reduced to something a filesystem and a human both accept. */
function slug(title: string): string {
  return title
    .normalize("NFKD")
    // Strip the combining marks NFKD just split off, so "café" becomes "cafe"
    // rather than "cafe" plus an invisible accent the filesystem keeps.
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^\w\s-]/g, "")
    .trim()
    .replace(/\s+/g, "_")
    .slice(0, 60)
    .replace(/_+$/, "");
}

export type ImageSaveResult = { saved: number; failed: number };

/**
 * Paste-a-link gallery capture: analyze a post URL into the pictures it holds,
 * then hand the chosen ones to the ordinary download queue.
 *
 * There is no job list here, and deliberately so. A picture is one small file
 * the existing queue already fetches, retries, and reveals — giving images a
 * parallel job system would have meant a second Downloads list for the user to
 * watch. What this hook owns is only the part the queue has no opinion on:
 * which of a post's pictures the user actually wants.
 */
export function useImages() {
  const [album, setAlbum] = useState<ImageAlbum | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const analysisGeneration = useRef(0);

  // The headers the album was resolved with. A login-gated post needs the same
  // session to fetch the pictures that it needed to list them, so they travel
  // from analyze to download the way the video view's do.
  const headersRef = useRef<[string, string][] | undefined>(undefined);

  const selectedItems = useMemo(
    () => (album ? album.items.filter((item) => selected.has(item.id)) : []),
    [album, selected],
  );

  const analyze = useCallback(async (url: string, requestHeaders?: [string, string][]) => {
    const generation = ++analysisGeneration.current;
    setError(null);
    setAlbum(null);
    setSelected(new Set());
    headersRef.current = requestHeaders;
    setAnalyzing(true);
    try {
      const found = await engine.analyzeImages(url, requestHeaders);
      if (generation !== analysisGeneration.current) return null;
      setAlbum(found);
      // Everything starts selected: a post's pictures are usually wanted
      // together, and deselecting two is less work than selecting eight.
      setSelected(new Set(found.items.map((item) => item.id)));
      return found;
    } catch (e) {
      if (generation !== analysisGeneration.current) return null;
      setError(e instanceof Error ? e.message : String(e));
      return null;
    } finally {
      if (generation === analysisGeneration.current) setAnalyzing(false);
    }
  }, []);

  const toggle = useCallback((id: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  }, []);

  const selectAll = useCallback(() => {
    setSelected(new Set(album?.items.map((item) => item.id) ?? []));
  }, [album]);

  const selectNone = useCallback(() => setSelected(new Set()), []);

  /**
   * Queue the given items (default: the selected ones).
   *
   * Each is enqueued individually rather than in one batch call, so one dead
   * link in a carousel costs that picture and not the other seven.
   */
  const save = useCallback(
    async (items?: ImageItem[], autoDiscord?: boolean): Promise<ImageSaveResult> => {
      const target = items ?? selectedItems;
      if (!album || target.length === 0 || !hasEngine()) {
        return { saved: 0, failed: 0 };
      }

      setError(null);
      setSaving(true);
      let saved = 0;
      let failed = 0;
      try {
        for (const item of target) {
          // The index is the item's place in the *album*, not in the
          // selection, so filenames stay stable whichever subset is saved.
          const index = album.items.indexOf(item);
          const headers = mergeHeaders(headersRef.current, item.headers);
          const name = itemFilename(album, item, index);
          const format = item.formats?.[0];
          try {
            if (item.kind === "video" && format) {
              // Clips take the video pipeline, not the plain queue. It is the
              // only one that fetches a manifest, pairs a split audio track and
              // muxes the result — and a post's clip is routinely all three.
              // The title carries the filename because that is what the video
              // job names its output from.
              await engine.startVideoDownload(
                stripExtension(name),
                album.sourceUrl,
                { ...format, downloadOptions: { ...format.downloadOptions, thumbnail: item.thumbnail } },
                undefined,
                headers,
                autoDiscord,
              );
            } else {
              await engine.add(item.url, undefined, undefined, name, headers, autoDiscord);
            }
            saved += 1;
          } catch {
            failed += 1;
          }
        }
      } finally {
        setSaving(false);
      }

      if (failed > 0 && saved === 0) {
        setError("The engine would not accept any of these links.");
      }
      return { saved, failed };
    },
    [album, selectedItems],
  );

  return {
    album,
    analyzing,
    saving,
    error,
    selected,
    selectedItems,
    clearError: () => setError(null),
    clear: () => {
      ++analysisGeneration.current;
      setAnalyzing(false);
      headersRef.current = undefined;
      setAlbum(null);
      setSelected(new Set());
      setError(null);
    },
    analyze,
    toggle,
    selectAll,
    selectNone,
    save,
  };
}

/**
 * The item's own headers layered over the album-wide ones, item first.
 *
 * Mirrors the engine's merge: a Facebook `lookaside` URL is only served to the
 * crawler `User-Agent`, so the item's value has to win over the session's.
 */
function mergeHeaders(
  base: [string, string][] | undefined,
  item: [string, string][] | undefined,
): [string, string][] | undefined {
  if (!item?.length) return base;
  const merged: [string, string][] = [...item];
  for (const [name, value] of base ?? []) {
    if (!merged.some(([existing]) => existing.toLowerCase() === name.toLowerCase())) {
      merged.push([name, value]);
    }
  }
  return merged;
}
