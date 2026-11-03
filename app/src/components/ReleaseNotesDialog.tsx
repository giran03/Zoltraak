import { useEffect, useId, useRef, useState } from "react";
import { BookOpen, Close } from "./Icons";
import { Button } from "./Controls";
import { TooltipGroup } from "./Tooltip";
import { currentRelease, hasNewRelease, RELEASE_SEEN_KEY, releaseVersion } from "../lib/releaseNotes";
import { safeStorage } from "../lib/safeStorage";

export function ReleaseNotesDialog({ autoOpen = false, onClose }: { autoOpen?: boolean; onClose?: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  const description = useId();
  const content = useId();
  const [detailed, setDetailed] = useState(false);
  const [open, setOpen] = useState(() => !!currentRelease && (!autoOpen || hasNewRelease(releaseVersion, safeStorage.getItem(RELEASE_SEEN_KEY))));

  useEffect(() => {
    const element = dialog.current;
    if (!open || !element) return;
    const previous = document.activeElement as HTMLElement | null;
    element.showModal();
    const frame = requestAnimationFrame(() => element.classList.add("is-open"));
    return () => {
      cancelAnimationFrame(frame);
      element.classList.remove("is-open");
      element.close();
      if (previous?.isConnected) previous.focus();
    };
  }, [open]);

  const dismiss = () => {
    if (!open) return;
    // A downgrade must not forget that a newer version has already been seen.
    if (hasNewRelease(releaseVersion, safeStorage.getItem(RELEASE_SEEN_KEY))) safeStorage.setItem(RELEASE_SEEN_KEY, releaseVersion);
    setOpen(false);
    onClose?.();
  };
  if (!open || !currentRelease) return null;

  return <dialog ref={dialog} aria-labelledby={title} aria-describedby={description}
    className="release-notes-dialog t-modal" onCancel={event => { event.preventDefault(); dismiss(); }}
    onClick={event => {
      if (event.target !== event.currentTarget) return;
      const rect = event.currentTarget.getBoundingClientRect();
      if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) dismiss();
    }}>
    <header className="flex items-start gap-3 border-b border-edge px-5 py-4 sm:px-6">
      <div className="min-w-0 flex-1">
        <h2 id={title} className="text-[18px] font-semibold text-ink">What's new in Zoltraak <span className="whitespace-nowrap font-mono text-[13px] text-ink-dim">v{releaseVersion}</span></h2>
        <p id={description} className="mt-2 max-w-[65ch] text-[13px] leading-relaxed text-ink-dim">{currentRelease.summary}</p>
      </div>
      <TooltipGroup className="shrink-0 gap-1">
        <button type="button" aria-label="Detailed changelog" aria-pressed={detailed} aria-controls={content}
          data-tooltip={detailed ? "Show brief notes" : "Show detailed changelog"}
          onClick={() => setDetailed(value => !value)}
          className={`t-tt-trigger focus-ring rounded-hw p-2 hover:bg-recess ${detailed ? "bg-recess text-route" : "text-ink-dim hover:text-ink"}`}><BookOpen className="size-4" /></button>
        <button type="button" aria-label="Close release notes" data-tooltip="Close" onClick={dismiss}
          className="t-tt-trigger focus-ring rounded-hw p-2 text-ink-dim hover:bg-recess hover:text-ink"><Close className="size-4" /></button>
      </TooltipGroup>
    </header>
    <div id={content} className="min-h-0 flex-1 overflow-y-auto px-5 py-4 sm:px-6">
      <p className="mb-3 font-mono text-[11px] uppercase tracking-wider text-ink-dim">{detailed ? "Detailed changelog" : "Release highlights"}</p>
      <ul className="list-disc space-y-4 pl-4 text-ink-dim">
        {currentRelease.changes.map(change => <li key={change.brief}>
          <p className="text-[13px] font-medium leading-relaxed text-ink">{change.brief}</p>
          {detailed && <ul className="mt-2 list-disc space-y-1.5 pl-4 text-[12px] leading-relaxed text-ink-dim">
            {change.details.map(detail => <li key={detail}>{detail}</li>)}
          </ul>}
        </li>)}
      </ul>
    </div>
    <footer className="flex flex-wrap items-center justify-between gap-3 border-t border-edge bg-recess/40 px-5 py-4 sm:px-6">
      <p className="text-[11px] text-ink-dim">Read these notes again in Settings / About.</p>
      <Button onClick={dismiss}>Continue</Button>
    </footer>
  </dialog>;
}
