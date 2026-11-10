import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Close, DownloadGlyph } from "./Icons";
import { Lamp } from "./Panel";

export type ToastItem = {
  id: string;
  type: "success" | "error" | "info";
  title: string;
  message: string;
  /** Entry IDs for reopening this notification from history. */
  downloadIds?: string[];
  /** An offer attached to the toast — "Undo", typically. */
  action?: { label: string; onClick: () => void };
  /** A second explicit choice, used when dismissing is not itself the action. */
  secondaryAction?: { label: string; onClick: () => void };
  /** Replaces the close glyph when dismissing is a meaningful choice. */
  dismissLabel?: string;
  /** Overrides the six-second lifetime with a positive, finite duration. */
  durationMs?: number;
};

const TONE = {
  success: { lamp: "bg-clear", ink: "text-clear" },
  error: { lamp: "bg-danger", ink: "text-danger" },
  info: { lamp: "bg-route", ink: "text-route" },
} as const;

/**
 * Drawn glyphs rather than platform emoji: the rest of the app draws its icons
 * on one 24px grid at a 1.5px stroke, and OS-rendered colour emoji beside them
 * reads as a different product.
 */
function ToastIcon({ type, className }: { type: ToastItem["type"]; className?: string }) {
  if (type === "success") return <Check className={className} />;
  if (type === "error") return <Close className={className} />;
  return <DownloadGlyph className={className} />;
}

function ToastCard({
  toast,
  folded,
  onDismiss,
  onUnfold,
}: {
  toast: ToastItem;
  /** Older toasts fold into a narrow strip beside the one in focus. */
  folded: boolean;
  onDismiss: (id: string) => void;
  onUnfold: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const tone = TONE[toast.type];

  useEffect(() => {
    const frame = requestAnimationFrame(() => setOpen(true));
    return () => cancelAnimationFrame(frame);
  }, []);

  const handleDismiss = useCallback(() => {
    setOpen(false);
    const rawDur =
      typeof window !== "undefined"
        ? getComputedStyle(document.documentElement).getPropertyValue("--toast-close")
        : "";
    const closeMs = parseFloat(rawDur) || 250;
    setTimeout(() => onDismiss(toast.id), closeMs);
  }, [onDismiss, toast.id]);

  return (
    <div
      className={`t-toast t-toast-fold ${open ? "is-open" : ""} ${folded ? "is-folded" : ""} plate lift toast-glass pointer-events-auto relative shrink-0 overflow-hidden rounded-hw-lg`}
    >
      {folded ? (
        <button
          type="button"
          title={toast.title}
          aria-label={`Show notification: ${toast.title}`}
          onClick={() => onUnfold(toast.id)}
          className="focus-ring absolute inset-0 flex flex-col items-center gap-2 px-1 pt-3 text-ink-dim transition-colors hover:bg-recess/60 hover:text-ink"
        >
          <Lamp fill={tone.lamp} size={7} />
          <span className={tone.ink}>
            <ToastIcon type={toast.type} className="size-3.5" />
          </span>
        </button>
      ) : (
        <div className="flex min-w-[20rem] items-start gap-3 px-3.5 py-3">
          <span className="mt-[3px] flex shrink-0 items-center gap-2">
            <Lamp fill={tone.lamp} size={7} />
            <span className={tone.ink}>
              <ToastIcon type={toast.type} className="size-3.5" />
            </span>
          </span>

          <div className="min-w-0 flex-1">
            <p className="text-[12px] text-ink">{toast.title}</p>
            <p data-selectable className="mt-0.5 break-words text-[12px] leading-relaxed text-ink-dim">
              {toast.message}
            </p>
            {toast.action && (
              <div className="mt-2 flex flex-wrap gap-2">
                <button
                  type="button"
                  onClick={() => {
                    toast.action?.onClick();
                    handleDismiss();
                  }}
                  className="focus-ring legend rounded-hw border border-edge-strong px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-recess hover:text-ink"
                >
                  {toast.action.label}
                </button>
                {toast.secondaryAction && (
                  <button
                    type="button"
                    onClick={() => {
                      toast.secondaryAction?.onClick();
                      handleDismiss();
                    }}
                    className="focus-ring legend rounded-hw border border-edge-strong px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-recess hover:text-ink"
                  >
                    {toast.secondaryAction.label}
                  </button>
                )}
              </div>
            )}
          </div>

          <button
            type="button"
            aria-label={toast.dismissLabel ?? "Dismiss notification"}
            onClick={handleDismiss}
            className="focus-ring shrink-0 rounded-hw p-0.5 text-ink-faint transition-colors hover:text-ink"
          >
            {toast.dismissLabel ?? <Close className="size-3.5" />}
          </button>
        </div>
      )}
    </div>
  );
}

/**
 * Indications posted to the desk.
 *
 * A plate with a lamp, not a floating glass card: the toast is the same
 * material as everything else on the panel, so it reads as part of the machine
 * rather than an overlay borrowed from a web app.
 */
export function ToastContainer({
  toasts,
  onDismiss,
  hidden = false,
}: {
  toasts: ToastItem[];
  onDismiss: (id: string) => void;
  /** History owns this corner while open; the shared expiry timers keep running. */
  hidden?: boolean;
}) {
  const stackRef = useRef<HTMLDivElement>(null);
  // The newest toast takes focus; clicking a folded strip moves focus to it.
  const [pickedId, setPickedId] = useState<string | null>(null);
  const newestId = toasts[0]?.id;
  useEffect(() => setPickedId(null), [newestId]);
  const focusId = toasts.some((toast) => toast.id === pickedId) ? pickedId : newestId;
  useEffect(() => {
    const stack = stackRef.current;
    if (!stack || hidden || !toasts.length || typeof stack.showPopover !== "function") return;
    stack.showPopover();
    return () => { if (stack.isConnected && stack.matches(":popover-open")) stack.hidePopover(); };
  }, [hidden, toasts.length]);
  if (hidden || toasts.length === 0) return null;

  return (
    <div
      ref={stackRef}
      popover={typeof HTMLElement !== "undefined" && "popover" in HTMLElement.prototype ? "manual" : undefined}
      role="status"
      aria-live="polite"
      className="pointer-events-none fixed bottom-auto left-4 right-4 top-14 z-[9999] m-0 flex w-auto flex-row items-stretch justify-end gap-1.5 border-0 bg-transparent p-0 sm:left-auto sm:right-5"
    >
      {/* Oldest on the left, so the stack folds toward the edge it grew from. */}
      {[...toasts].reverse().map((toast) => (
        <ToastCard
          key={toast.id}
          toast={toast}
          folded={toast.id !== focusId}
          onDismiss={onDismiss}
          onUnfold={setPickedId}
        />
      ))}
    </div>
  );
}
