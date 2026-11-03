/**
 * Selection arithmetic for a list of rows.
 *
 * Kept out of the view because this is the part with edge cases — an anchor
 * that has since been filtered away, a range dragged upward, a click that
 * should toggle rather than extend — and none of them need a DOM to exercise.
 */

export type Selectable = { id: string };

/**
 * The ids between two rows, inclusive, in either direction.
 *
 * Returns an empty list when either end is no longer present: a stale anchor
 * should fall back to a plain click, never silently select from index 0.
 */
export function rangeBetween(rows: readonly Selectable[], fromId: string, toId: string): string[] {
  const from = rows.findIndex((row) => row.id === fromId);
  const to = rows.findIndex((row) => row.id === toId);
  if (from < 0 || to < 0) return [];

  const [lo, hi] = from <= to ? [from, to] : [to, from];
  return rows.slice(lo, hi + 1).map((row) => row.id);
}

/**
 * Apply a click to a selection.
 *
 * Shift extends from `anchor` the way every file list does; a plain click
 * toggles the row and becomes the new anchor. The returned anchor is what the
 * next shift-click measures from — it deliberately does not move on a shift
 * click, so extending twice grows one range from the same origin instead of
 * walking it forward.
 */
export function applyClick({
  rows,
  selected,
  anchor,
  id,
  shiftKey,
}: {
  rows: readonly Selectable[];
  selected: ReadonlySet<string>;
  anchor: string | null;
  id: string;
  shiftKey: boolean;
}): { selected: Set<string>; anchor: string | null } {
  if (shiftKey && anchor) {
    const range = rangeBetween(rows, anchor, id);
    if (range.length > 0) {
      return { selected: new Set([...selected, ...range]), anchor };
    }
  }

  const next = new Set(selected);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return { selected: next, anchor: id };
}

/**
 * Drop ids that are no longer on screen.
 *
 * A selection that outlives its rows makes a bulk action operate on things the
 * user cannot see. Returns the original set when nothing changed, so callers
 * can skip a re-render.
 */
export function pruneSelection(
  selected: ReadonlySet<string>,
  rows: readonly Selectable[],
): Set<string> {
  const visible = new Set(rows.map((row) => row.id));
  const next = new Set([...selected].filter((id) => visible.has(id)));
  return next;
}
