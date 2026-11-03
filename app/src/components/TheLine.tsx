import { memo, useEffect, useRef, useState } from "react";
import { aspectOf } from "../lib/aspect";
import { formatBytes, formatEta } from "../lib/format";
import { occupancyOf, type Occupancy } from "../lib/occupancy";
import type { QueueItem } from "../lib/snapshots";
import { Lamp, Legend } from "./Panel";

/**
 * THE LINE.
 *
 * A dispatcher's track diagram. Every transfer still on the railway takes a
 * lane, and each lane is cut into as many blocks as the transfer has real
 * connections — sixteen connections is sixteen blocks. A block is a byte
 * range: it is occupied or it is clear, so blocks step and never tween.
 *
 * The proportion of lit blocks is exactly bytes-on-disk over total, and the
 * byte figures sit beside the lane so the exact truth is always one glance
 * away. What the diagram does NOT claim is which connection owns which block:
 * the engine reports an aggregate byte count and a connection count, not a
 * per-connection range map. Blocks therefore fill from the left in completion
 * order, and the flash marks a range completing — it is not, and must not be
 * described as, a work-stealing hand-over between two named connections.
 * Drawing that would need range ownership the engine does not send.
 *
 * Lane height is the whole answer to a queue that flips between one 8 GB ISO
 * and sixty files draining overnight: one lane fills the strip, twelve stack
 * compact, and nothing about it is a mode the user has to switch.
 */

/** Finished and failed work has left the railway; its block is released. */
function onTheLine(item: QueueItem): boolean {
  return item.state !== "finished" && item.state !== "failed";
}

const MAX_LANES = 10;

/**
 * THE LINE is the monumental element on this surface, so the track carries the
 * scale rather than a headline number. The aggregate rate lives small in the
 * panel header, where the contract put it and where it cannot become the
 * total-speed hero the THESIS refuses.
 */
function laneHeight(count: number): number {
  if (count <= 1) return 72;
  if (count <= 3) return 44;
  if (count <= 6) return 26;
  return 16;
}

function describeOccupancy(item: QueueItem, occupancy: Occupancy): string {
  const total = item.totalBytes && item.totalBytes > 0 ? item.totalBytes : null;
  const size = total
    ? `${formatBytes(item.downloadedBytes)} of ${formatBytes(total)} on disk`
    : `${formatBytes(item.downloadedBytes)} on disk, total size unknown`;

  return occupancy.kind === "stream"
    ? `${item.filename}: ${size}, single stream`
    : `${item.filename}: ${size} across ${occupancy.blocks} byte ${
        occupancy.blocks === 1 ? "range" : "ranges"
      }`;
}

/**
 * The block that just lit gets one short throw.
 *
 * This marks a byte range completing — the count went up — and nothing more.
 * It deliberately does not claim to show a range moving between connections;
 * see the note at the top of this file for why that is not drawable from the
 * data the engine sends.
 */
function useNewlyLit(id: string, lit: number): boolean {
  const previous = useRef(lit);
  const [flash, setFlash] = useState(false);

  useEffect(() => {
    if (lit > previous.current) {
      setFlash(true);
      const timer = setTimeout(() => setFlash(false), 420);
      previous.current = lit;
      return () => clearTimeout(timer);
    }
    previous.current = lit;
  }, [lit, id]);

  return flash;
}

const Lane = memo(function Lane({
  item,
  height,
  compact,
}: {
  item: QueueItem;
  height: number;
  /** Tight tier: the lane keeps its identity and its due time, and drops the
   *  byte figures the register two inches below is already showing. */
  compact: boolean;
}) {
  const aspect = aspectOf(item.state);
  const total = item.totalBytes && item.totalBytes > 0 ? item.totalBytes : null;
  const occupancy = occupancyOf(item);

  const flash = useNewlyLit(item.id, occupancy.kind === "blocks" ? occupancy.lit : 0);

  return (
    <div className="flex items-center gap-2.5">
      {/* Identity first. A track diagram whose lanes are anonymous is a bar
          chart, and the tier where lanes get thin is exactly the tier where a
          dispatcher most needs to know which one is which. */}
      <Lamp fill={aspect.lamp} size={7} />
      <span
        data-selectable
        title={item.url}
        className={`describer shrink-0 truncate text-[11px] text-ink-dim ${
          compact ? "w-32" : "w-44"
        }`}
      >
        {item.filename}
      </span>

      <div
        className="t-resize flex min-w-0 flex-1 gap-[2px]"
        style={{ height }}
        role="img"
        aria-label={describeOccupancy(item, occupancy)}
      >
        {occupancy.kind === "stream" ? (
          /* One continuous stream. No cell divisions, because a video job has
             no byte ranges to divide. */
          <span className="block relative min-w-0 flex-1 overflow-hidden rounded-hw-sm bg-block-clear">
            <span
              className={`absolute inset-y-0 left-0 ${aspect.block} ${
                occupancy.fraction === null ? "opacity-40" : ""
              }`}
              style={{ width: `${(occupancy.fraction ?? 1) * 100}%` }}
            />
          </span>
        ) : (
          Array.from({ length: occupancy.blocks }, (_, index) => {
            const occupied = index < occupancy.lit;
            const isPartial = index === occupancy.lit && occupancy.partial > 0;
            const justLit = flash && index === occupancy.lit - 1;

            return (
              <span
                key={index}
                className={`block relative min-w-0 flex-1 overflow-hidden rounded-hw-sm ${
                  occupied ? aspect.block : "bg-block-clear"
                } ${justLit ? "animate-[throw_420ms_var(--ease-throw)]" : ""}`}
              >
                {isPartial && (
                  <span
                    className={`absolute inset-y-0 left-0 ${aspect.block} opacity-45`}
                    style={{ width: `${occupancy.partial * 100}%` }}
                  />
                )}
              </span>
            );
          })
        )}
      </div>

      <div
        className={`flex shrink-0 items-baseline justify-end gap-3 ${
          compact ? "w-16" : "w-44"
        }`}
      >
        {!compact && (
          <span data-measure className="describer text-[11px] text-ink-dim">
            {total
              ? `${formatBytes(item.downloadedBytes)} / ${formatBytes(total)}`
              : formatBytes(item.downloadedBytes)}
          </span>
        )}
        <span data-measure className={`describer text-[11px] ${aspect.ink}`}>
          {formatEta(item.etaSeconds)}
        </span>
      </div>
    </div>
  );
});

export const TheLine = memo(function TheLine({
  items,
  onShowActive,
}: {
  items: QueueItem[];
  /** Reveals the transfers past the lane cap in the register below. */
  onShowActive?: () => void;
}) {
  const running = items.filter((item) => item.state === "downloading");
  const active = items.filter(onTheLine);
  const finished = items.filter((item) => item.state === "finished");
  const failed = items.filter((item) => item.state === "failed");

  const onDisk = items.reduce((sum, item) => sum + (item.downloadedBytes || 0), 0);

  const shown = active.slice(0, MAX_LANES);
  const hidden = active.length - shown.length;
  const height = laneHeight(shown.length);
  const compact = shown.length > 6;

  return (
    <section aria-label="The line" className="shrink-0 px-6 pb-4 pt-3">
      <dl className="flex flex-wrap items-baseline gap-x-5 gap-y-1">
        <Tally label="running" value={running.length} tone="text-clear" />
        <Tally label="on the line" value={active.length} tone="text-ink" />
        {failed.length > 0 && <Tally label="failed" value={failed.length} tone="text-danger" />}
        <Tally label="finished" value={finished.length} tone="text-ink-dim" />
        <div className="flex flex-row-reverse items-baseline gap-2">
          <dt>
            <Legend>on disk</Legend>
          </dt>
          <dd data-measure className="describer text-[13px] text-ink-dim">
            {formatBytes(onDisk)}
          </dd>
        </div>
        <span className="h-px min-w-4 flex-1 bg-edge" />
      </dl>

      {shown.length > 0 && (
        <div className="mt-2.5 space-y-[3px]">
          {shown.map((item) => (
            <Lane key={item.id} item={item} height={height} compact={compact} />
          ))}
        </div>
      )}

      {/* A count of things you cannot see is only useful with a way to see
          them, so this is a control rather than a caption. */}
      {hidden > 0 && (
        <button
          type="button"
          onClick={onShowActive}
          className="focus-ring legend mt-2 rounded-hw px-1 py-0.5 text-[11px] text-ink-dim transition-colors hover:text-ink"
        >
          {hidden} more on the line — show in the register
        </button>
      )}
    </section>
  );
});

function Tally({ label, value, tone }: { label: string; value: number; tone: string }) {
  return (
    <div className="flex flex-row-reverse items-baseline gap-2">
      <dt>
        <Legend>{label}</Legend>
      </dt>
      <dd data-measure className={`describer text-[13px] ${tone}`}>
        {value}
      </dd>
    </div>
  );
}
