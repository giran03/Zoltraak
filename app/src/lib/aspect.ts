import type { DownloadState } from "./types";

/**
 * Signal aspects.
 *
 * The panel's state vocabulary, defined once so the lamp, the occupancy
 * blocks, the register row and the context menu can never disagree about what
 * a state looks like.
 *
 * The *labels* stay the engine's own words. A dispatcher's panel earns its
 * clarity from the lamp, not from renaming things the user already knows —
 * calling a paused download "HELD" would be the world obscuring the task,
 * which is the one thing an operate surface may never do.
 */
export type Aspect = {
  /** The word shown on the row. Engine vocabulary, uppercased. */
  label: string;
  /** Tailwind text colour token for the state word. */
  ink: string;
  /** Lamp fill. `null` means the lamp is out — finished work stops asking for
   *  attention, which is what makes a failure visible in a sixty-row queue. */
  lamp: string | null;
  /** Occupancy fill for this row's blocks. */
  block: string;
  /** A fault takes a filled slug rather than a lit lamp, so red-as-running and
   *  red-as-failed are separated by form and never by hue alone. */
  fault?: boolean;
};

export const ASPECT: Record<DownloadState, Aspect> = {
  downloading: {
    label: "DOWNLOADING",
    ink: "text-clear",
    lamp: "bg-clear",
    block: "bg-clear",
  },
  waiting: {
    label: "WAITING",
    ink: "text-caution",
    lamp: "bg-caution",
    block: "bg-caution",
  },
  paused: {
    label: "PAUSED",
    ink: "text-paused",
    lamp: "bg-paused",
    block: "bg-paused",
  },
  // Brass, not route-blue. Blue is the panel's chrome — the focus ring, the
  // active tab, row selection, the primary button — so spending it on an
  // aspect too would mean the one colour that is supposed to carry state also
  // carries "you clicked this".
  scheduled: {
    label: "SCHEDULED",
    ink: "text-brass",
    lamp: "bg-brass",
    block: "bg-brass",
  },
  // Finished work goes quiet, occupancy included: lighting a completed row's
  // whole strip in its aspect colour made the morning view of an overnight
  // queue brightest exactly where nothing needs attention.
  //
  // Quiet, not empty. A neutral fill still reads as occupied, so a finished
  // transfer stays distinguishable from a waiting one that has nothing on disk
  // — the two looked identical when this was `block-clear`.
  finished: {
    label: "FINISHED",
    ink: "text-ink-faint",
    lamp: null,
    block: "bg-edge-strong",
  },
  failed: {
    label: "FAILED",
    ink: "text-danger",
    lamp: "bg-danger",
    block: "bg-danger",
    fault: true,
  },
};

export function aspectOf(state: DownloadState): Aspect {
  return ASPECT[state] ?? ASPECT.waiting;
}
