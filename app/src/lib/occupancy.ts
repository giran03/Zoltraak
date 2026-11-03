import type { QueueItem } from "./snapshots";

/**
 * How a transfer's length of track is drawn.
 *
 * One place, because THE LINE and the register row draw the same download two
 * inches apart: when they disagreed about how many blocks a transfer has, the
 * same file was a 24-range job on one and a 16-range job on the other, and at
 * least one of them was lying.
 *
 * A block is a byte range. The count is the engine's real connection count and
 * nothing else — no padding a single-connection transfer up to a prettier
 * minimum, because a drawing that invents ranges is exactly the vagueness this
 * panel exists to refuse.
 */

/** Above this the cells stop being separable, so the strip stops subdividing. */
export const MAX_BLOCKS = 24;

export type Occupancy =
  /** A segmented transfer: N real byte ranges, `lit` of them complete. */
  | { kind: "blocks"; blocks: number; lit: number; partial: number }
  /**
   * One continuous stream with no byte-range resume — a video job. Drawing it
   * as blocks would claim segmentation the engine never performed.
   */
  | { kind: "stream"; fraction: number | null };

export function occupancyOf(item: QueueItem): Occupancy {
  const total = item.totalBytes && item.totalBytes > 0 ? item.totalBytes : null;
  const fraction = total ? Math.min(item.downloadedBytes / total, 1) : null;

  // A video job carries `connections: 1` as a placeholder and has no segmented
  // resume at all (see PRODUCT.md). It gets a continuous meter, not a track.
  if (item.isVideo) return { kind: "stream", fraction };

  const blocks = Math.max(1, Math.min(item.connections || 1, MAX_BLOCKS));
  const exact = fraction === null ? 0 : fraction * blocks;
  const lit = Math.floor(exact);

  return { kind: "blocks", blocks, lit, partial: exact - lit };
}
