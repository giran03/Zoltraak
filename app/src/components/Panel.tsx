import type { ReactNode } from "react";

/**
 * Panel primitives.
 *
 * The enamel panel replaces the old double-bezel: a plate is a machined face
 * with a hairline edge and a real cast shadow, a well is a recess cut into the
 * panel, and a lamp is a small lit cell in a dark housing. Nothing here nests
 * inside anything else of its own kind — a plate inside a plate is the card
 * stack the redesign exists to remove.
 */

/** A raised machined face. */
export function Plate({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  return <div className={`plate rounded-hw-lg ${className}`}>{children}</div>;
}

/** A well cut into the panel: inputs, readouts, lamp housings. */
export function Well({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  return <div className={`recess rounded-hw-lg ${className}`}>{children}</div>;
}

/**
 * An engraved legend. Condensed caps cut into the panel, the way a control
 * desk names its sections. This is a label for a control, never a kicker
 * floating above a heading.
 */
export function Legend({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <span className={`legend text-[11px] text-ink-faint ${className}`}>{children}</span>
  );
}

/**
 * A signal lamp.
 *
 * Switches. Never fades — a lamp is on or it is off, and a colour transition
 * would draw a state that does not exist. `fill` of `null` is an unlit
 * housing, which is exactly what a finished download gets.
 */
export function Lamp({
  fill,
  size = 8,
  className = "",
}: {
  fill: string | null;
  size?: number;
  className?: string;
}) {
  return (
    <span
      aria-hidden="true"
      style={{ width: size, height: size }}
      className={`inline-block shrink-0 rounded-hw-sm ring-1 ring-inset ring-black/25 ${
        fill ?? "bg-lamp-off"
      } ${className}`}
    />
  );
}

/**
 * A fault indication: a filled slug, not a lit lamp.
 *
 * Red does double duty on this panel — a held signal and a failure — so the
 * two are separated by form. A hold is a lamp; a fault is a stamped plate.
 */
export function FaultSlug({ children }: { children: ReactNode }) {
  return (
    <span className="legend inline-flex items-center rounded-hw bg-danger px-1.5 py-[2px] text-[11px] text-on-signal">
      {children}
    </span>
  );
}

/**
 * A section rule with its legend cut into it — how a panel divides regions.
 * Replaces the heading-inside-a-card pattern.
 */
export function LegendRule({
  children,
  right,
  className = "",
}: {
  children: ReactNode;
  right?: ReactNode;
  className?: string;
}) {
  return (
    <div className={`flex items-center gap-3 ${className}`}>
      <Legend>{children}</Legend>
      <span className="h-px min-w-4 flex-1 bg-edge" />
      {right}
    </div>
  );
}
