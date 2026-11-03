import { isMediaPath } from "./media.ts";

/**
 * Files dropped onto the Convert tab.
 *
 * Two jobs, both kept out of the component so they can be tested without a
 * window: deciding which dropped names the converter can take, and asking the
 * desktop shell for the real paths of the files that were dropped.
 */

export type DroppedEntry = { name: string; isDirectory: boolean };

export type DropVerdict<T extends DroppedEntry = DroppedEntry> = {
  /** Folders and files of a supported type. Folders are filtered by the engine when it reads them. */
  accepted: T[];
  /** Names of files the converter cannot take. */
  unsupported: string[];
};

export function judgeDrop<T extends DroppedEntry>(entries: readonly T[]): DropVerdict<T> {
  const accepted: T[] = [];
  const unsupported: string[] = [];
  for (const entry of entries) {
    if (entry.isDirectory || isMediaPath(entry.name)) accepted.push(entry);
    else unsupported.push(entry.name);
  }
  return { accepted, unsupported };
}

/** What can be converted, as the unsupported-file message lists it. */
export const SUPPORTED_FORMATS: readonly { kind: string; formats: string }[] = [
  { kind: "Video", formats: "MP4, MKV, MOV, WebM, AVI, FLV, TS, MPEG, WMV, 3GP and similar" },
  { kind: "Audio", formats: "MP3, M4A, AAC, FLAC, WAV, OGG, Opus, WMA, AIFF" },
  { kind: "Images and animations", formats: "PNG, JPG, WebP, GIF, BMP, TIFF, APNG" },
];

// ----------------------------------------------------------- real paths

/** Must match `file_drop.rs`. */
const DROP_MESSAGE = "zoltraak:drop-paths";
const DROPPED_PATHS_EVENT = "zoltraak://dropped-paths";
const PATHS_TIMEOUT_MS = 4000;

type WebView2 = { postMessageWithAdditionalObjects?: (message: string, objects: unknown[]) => void };

/** Whether this window can turn dropped files into paths at all. */
export function canResolveDroppedPaths(): boolean {
  const webview = (globalThis as { chrome?: { webview?: WebView2 } }).chrome?.webview;
  return typeof webview?.postMessageWithAdditionalObjects === "function" && "__TAURI_INTERNALS__" in globalThis;
}

/**
 * The paths of dropped files, or null when this window cannot say.
 *
 * The window leaves native drag and drop off so browser links and pictures
 * still arrive as HTML drops, so a dropped file is a `File` without a path. The
 * desktop shell listens for those same files posted with a marker message and
 * answers with their paths.
 */
export async function resolveDroppedPaths(files: readonly File[]): Promise<string[] | null> {
  const direct = files.map((file) => (file as File & { path?: unknown }).path).filter((path): path is string => typeof path === "string" && path.length > 0);
  if (files.length > 0 && direct.length === files.length) return direct;
  if (!canResolveDroppedPaths()) return null;
  const webview = (globalThis as unknown as { chrome: { webview: Required<WebView2> } }).chrome.webview;
  const { listen } = await import("@tauri-apps/api/event");
  return new Promise<string[]>((resolve) => {
    let finished = false;
    let stop: (() => void) | undefined;
    const finish = (paths: string[]) => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      stop?.();
      resolve(paths);
    };
    const timer = setTimeout(() => finish([]), PATHS_TIMEOUT_MS);
    listen<string[]>(DROPPED_PATHS_EVENT, (event) => finish(Array.isArray(event.payload) ? event.payload : []))
      .then((unlisten) => {
        stop = unlisten;
        if (finished) unlisten();
        else webview.postMessageWithAdditionalObjects(DROP_MESSAGE, [...files]);
      })
      .catch(() => finish([]));
  });
}
