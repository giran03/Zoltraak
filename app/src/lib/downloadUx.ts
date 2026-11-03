import { COLUMN_IDS, fractionOf, type ColumnId } from "./columns.ts";
import { extensionOf } from "./media.ts";
import type { QueueItem } from "./snapshots";

export type BatchResult = { added: string[]; failed: { url: string; error: string }[] };
export type AddResult = boolean | BatchResult;
export type BatchEntry = { raw: string; url: string | null; status: "ready" | "duplicate" | "existing" | "invalid" };

export function reviewLinks(text: string, existingUrls: string[] = []): BatchEntry[] {
  const canonical = (raw: string) => {
    try { const url = new URL(raw); return /^https?:\/\//i.test(raw) && ["http:", "https:"].includes(url.protocol) ? url.href : null; }
    catch { return null; }
  };
  const existing = new Set(existingUrls.map(canonical).filter(Boolean));
  const seen = new Set<string>();
  return text.split(/\r?\n/).filter(line => !line.trim().startsWith("#")).join("\n").split(/[\s,]+/).filter(Boolean).map(raw => {
    const url = canonical(raw.replace(/^[<("']+|[>)"',.]+$/g, ""));
    const status = !url ? "invalid" : seen.has(url) ? "duplicate" : existing.has(url) ? "existing" : "ready";
    if (url) seen.add(url);
    return { raw, url, status };
  });
}

export async function addLinks(urls: string[], add: (url: string) => Promise<unknown>): Promise<BatchResult> {
  const result: BatchResult = { added: [], failed: [] };
  for (const url of [...new Set(urls)]) {
    try { await add(url); result.added.push(url); }
    catch (error) { result.failed.push({ url, error: error instanceof Error ? error.message : String(error) }); }
  }
  return result;
}

export type RecoveryKind = "retry" | "signin" | "folder" | "extract";
export function recoveryFor(item: Pick<QueueItem, "error" | "isVideo" | "isMusic">) {
  const error = (item.error ?? "").toLowerCase();
  if (/disk|no space|permission denied|access (?:is )?denied|read.only|destination|directory|folder|os error (?:5|13|28)/.test(error))
    return { kind: "folder" as const, reason: "The download folder is full or cannot be written to.", label: "Choose another folder" };
  if (/401|403|sign.?in|login|cookies?|session|authentication|unauthorized|token.*expired/.test(error))
    return { kind: "signin" as const, reason: "This site may need a fresh browser sign-in. Share it through the extension, then retry.", label: "Share sign-in" };
  if (!item.isMusic && /extract|unsupported|no (?:playable|supported|video)|no formats|manifest|signature|decipher|yt-dlp/.test(error))
    return { kind: "extract" as const, reason: "The source could not be extracted. Try the alternative video extractor.", label: "Try another extraction method" };
  return { kind: "retry" as const, reason: /timeout|timed out|network|connection|429|503|502|504/.test(error)
    ? "The connection failed or the site is busy. Try the download again."
    : "The download did not finish. Retry it or review the details below.", label: "Retry download" };
}

export const DOWNLOAD_FILTERS = ["all", "downloading", "waiting", "paused", "finished", "failed", "scheduled", "videos", "music"] as const;
export type DownloadFilter = typeof DOWNLOAD_FILTERS[number];
export const SORTS = ["date", "name", "size", "speed", "progress"] as const;
export const DOWNLOAD_GROUPS = ["none", "type", "status", "source", "folder"] as const;
export type DownloadGrouping = typeof DOWNLOAD_GROUPS[number];
export type DownloadView = { query: string; filter: DownloadFilter; sort: typeof SORTS[number]; direction: "asc" | "desc"; groupBy: DownloadGrouping; density: "compact" | "comfortable"; columnOrder: ColumnId[] };
export const VIEW_KEY = "zoltraak.download-view.v1";
export const DEFAULT_VIEW: DownloadView = { query: "", filter: "all", sort: "date", direction: "desc", groupBy: "none", density: "compact", columnOrder: [...COLUMN_IDS] };
export function normalizeView(raw: unknown): DownloadView {
  const value = raw && typeof raw === "object" ? raw as Partial<DownloadView> : {};
  const columnOrder = Array.isArray(value.columnOrder) ? value.columnOrder.filter(id => COLUMN_IDS.includes(id)) : [];
  return { query: typeof value.query === "string" ? value.query : "", filter: DOWNLOAD_FILTERS.includes(value.filter!) ? value.filter! : "all",
    sort: SORTS.includes(value.sort!) ? value.sort! : "date", direction: value.direction === "asc" ? "asc" : "desc",
    groupBy: DOWNLOAD_GROUPS.includes(value.groupBy!) ? value.groupBy! : "none",
    density: value.density === "comfortable" ? "comfortable" : "compact", columnOrder: [...new Set([...columnOrder, ...COLUMN_IDS])] };
}
export function readDownloadView(): DownloadView {
  try { return normalizeView(JSON.parse(localStorage.getItem(VIEW_KEY) ?? "null")); }
  catch { return normalizeView(null); }
}

export type DownloadGroup = { key: string; label: string; items: QueueItem[] };
const STATUS_GROUPS = ["downloading", "waiting", "scheduled", "paused", "failed", "finished"] as const;
const STATUS_LABELS = { downloading: "Downloading", waiting: "Waiting", scheduled: "Scheduled", paused: "Paused", failed: "Failed", finished: "Finished" };

/** Groups a sorted list without changing the order of rows within each group. */
export function groupDownloads(items: QueueItem[], mode: DownloadGrouping): DownloadGroup[] {
  if (mode === "none") return items.length ? [{ key: "none", label: "", items }] : [];
  const groups = new Map<string, DownloadGroup>();
  for (const item of items) {
    let label: string, value: string;
    switch (mode) {
      case "type": {
        const extension = extensionOf(item.path || item.filename);
        label = /^[a-z0-9]{1,10}$/i.test(extension) ? extension.toUpperCase() : item.isMusic ? "Audio" : item.isVideo ? "Video" : "No extension";
        value = label.toLowerCase();
        break;
      }
      case "status": value = item.state; label = STATUS_LABELS[item.state]; break;
      case "source":
        try { label = new URL(item.url).hostname.toLowerCase() || "Unknown source"; }
        catch { label = "Unknown source"; }
        value = label;
        break;
      case "folder": {
        const path = item.path || "";
        const separator = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
        const directory = item.directory || (separator >= 0 ? path.slice(0, separator + 1) : "");
        label = directory || "No destination";
        value = directory.replaceAll("\\", "/").replace(/\/+$/, "") || directory || "No destination";
        if (/^[a-z]:/i.test(value) || value.startsWith("//")) value = value.toLowerCase();
        break;
      }
    }
    const key = `${mode}:${value}`;
    const group = groups.get(key);
    if (group) group.items.push(item);
    else groups.set(key, { key, label, items: [item] });
  }
  return [...groups.values()].sort((a, b) => mode === "status"
    ? STATUS_GROUPS.indexOf(a.items[0]!.state) - STATUS_GROUPS.indexOf(b.items[0]!.state)
    : a.label.localeCompare(b.label, undefined, { numeric: true, sensitivity: "base" }));
}
export function moveColumn(order: ColumnId[], id: ColumnId, offset: -1 | 1): ColumnId[] {
  const next = [...order]; const from = next.indexOf(id); const to = from + offset;
  if (from < 0 || to < 0 || to >= next.length) return order;
  [next[from], next[to]] = [next[to]!, next[from]!];
  return next;
}
export function sortDownloads(items: QueueItem[], view: Pick<DownloadView, "sort" | "direction">): QueueItem[] {
  const value = (item: QueueItem): string | number | null => {
    switch (view.sort) {
      case "name": return item.filename;
      case "size": return item.totalBytes;
      case "speed": return item.bytesPerSecond;
      case "progress": return fractionOf(item);
      default: { const date = Date.parse(item.createdAt ?? ""); return Number.isFinite(date) ? date : null; }
    }
  };
  return [...items].sort((a, b) => {
    const av = value(a), bv = value(b);
    if (av == null) return bv == null ? 0 : 1;
    if (bv == null) return -1;
    const comparison = typeof av === "string" && typeof bv === "string" ? av.localeCompare(bv, undefined, { numeric: true, sensitivity: "base" }) : Number(av) - Number(bv);
    return view.direction === "asc" ? comparison : -comparison;
  });
}
