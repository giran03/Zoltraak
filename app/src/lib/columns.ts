/**
 * The register's columns.
 *
 * One table of metadata, read by both the header legends and the row cells, so
 * a column can never be a different width above than it is below. The cells
 * themselves are rendered by `DownloadRow` — the widths, the labels and the
 * default set live here, where the header can reach them too.
 *
 * Deliberately free of React and of the engine bridge, so it can be unit
 * tested on its own.
 */
import { extensionOf } from "./media.ts";
import type { QueueItem } from "./snapshots";

export type ColumnId =
  | "aspect"
  | "occupancy"
  | "progress"
  | "size"
  | "remaining"
  | "rate"
  | "eta"
  | "connections"
  | "added"
  | "kind"
  | "type"
  | "host"
  | "folder"
  | "priority"
  | "source";

export type ColumnSpec = {
  id: ColumnId;
  /** The header legend. Lowercase — the header uppercases it. */
  label: string;
  /** What the header menu calls it, when the legend is too terse to pick from. */
  menuLabel: string;
  /** Tailwind width, applied identically to the legend and the cell. */
  width: string;
  /**
   * The breakpoint below which the column folds away regardless of choice.
   *
   * Choosing a column is a statement about what matters, not a promise that
   * six of them fit in a 900px window — past a point the description is the
   * one cell that must not be crushed, so the widest columns go first.
   */
  show: string;
  align: "left" | "right";
  /** Kept out of the default set because it is rarely the thing being read. */
  optional?: boolean;
};

/**
 * Every column the register can show, in the order it shows them.
 *
 * Order is fixed rather than user-sortable: the visible set is the choice on
 * offer, and a row whose columns move around between sessions is harder to
 * read than one with a column too many.
 */
export const COLUMNS: ColumnSpec[] = [
  { id: "aspect", label: "aspect", menuLabel: "Aspect", width: "w-24", show: "hidden md:block", align: "left" },
  { id: "kind", label: "kind", menuLabel: "Kind", width: "w-16", show: "hidden lg:block", align: "left", optional: true },
  { id: "type", label: "type", menuLabel: "File type", width: "w-16", show: "hidden lg:block", align: "left", optional: true },
  { id: "occupancy", label: "occupancy", menuLabel: "Occupancy track", width: "w-28", show: "hidden lg:block", align: "left" },
  { id: "progress", label: "done", menuLabel: "Percent done", width: "w-14", show: "hidden md:block", align: "right", optional: true },
  { id: "size", label: "on disk / nominal", menuLabel: "On disk / nominal", width: "w-36", show: "hidden sm:block", align: "right" },
  { id: "remaining", label: "remaining", menuLabel: "Remaining", width: "w-24", show: "hidden xl:block", align: "right", optional: true },
  { id: "rate", label: "rate", menuLabel: "Rate", width: "w-24", show: "hidden md:block", align: "right" },
  { id: "eta", label: "due", menuLabel: "Due (ETA)", width: "w-20", show: "hidden lg:block", align: "right" },
  { id: "connections", label: "conn", menuLabel: "Connections", width: "w-14", show: "hidden xl:block", align: "right", optional: true },
  { id: "added", label: "added", menuLabel: "Date added", width: "w-28", show: "hidden lg:block", align: "right" },
  { id: "priority", label: "prio", menuLabel: "Priority", width: "w-12", show: "hidden xl:block", align: "right", optional: true },
  { id: "host", label: "host", menuLabel: "Host", width: "w-36", show: "hidden xl:block", align: "left", optional: true },
  { id: "folder", label: "folder", menuLabel: "Destination folder", width: "w-40", show: "hidden lg:block", align: "left", optional: true },
  { id: "source", label: "source", menuLabel: "Source URL", width: "w-56", show: "hidden lg:block", align: "left", optional: true },
];

export const COLUMN_IDS = COLUMNS.map((column) => column.id);

/** What the register shows before anyone has chosen otherwise. */
export const DEFAULT_COLUMNS: ColumnId[] = COLUMNS.filter((c) => !c.optional).map((c) => c.id);

/** Unknown ids dropped, order forced back to `COLUMNS`. */
export function normalizeColumns(raw: unknown): ColumnId[] {
  if (!Array.isArray(raw)) return [...DEFAULT_COLUMNS];
  const wanted = new Set(raw.filter((id): id is ColumnId => COLUMN_IDS.includes(id as ColumnId)));
  return COLUMN_IDS.filter((id) => wanted.has(id));
}

/** The host a download came from, without the `www.` nobody reads. */
export function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return "—";
  }
}

/** The row's own extension, from whichever of path or filename exists. */
export function typeOf(item: QueueItem): string {
  const extension = extensionOf(item.path || item.filename);
  return extension ? extension.toUpperCase() : "—";
}

/** Which pipeline the row came from. */
export function kindOf(item: QueueItem): string {
  if (item.isVideo) return "video";
  if (item.isMusic) return "music";
  return "file";
}

/** Completion in `0..=1`, or `null` when the total size is unknown. */
export function fractionOf(item: QueueItem): number | null {
  if (item.state === "finished") return 1;
  const total = item.totalBytes && item.totalBytes > 0 ? item.totalBytes : null;
  if (total === null) return null;
  return Math.min(Math.max(item.downloadedBytes / total, 0), 1);
}

/** Bytes still to fetch, or `null` when the total size is unknown. */
export function remainingOf(item: QueueItem): number | null {
  if (item.state === "finished") return 0;
  const total = item.totalBytes && item.totalBytes > 0 ? item.totalBytes : null;
  if (total === null) return null;
  return Math.max(total - item.downloadedBytes, 0);
}
