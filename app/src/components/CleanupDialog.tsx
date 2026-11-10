import { useEffect, useId, useRef } from "react";
import { Button } from "./Controls";

/** Asks before entries for missing or failed downloads are removed from the list. */
export function CleanupDialog({
  missing,
  failed,
  onConfirm,
  onClose,
}: {
  missing: number;
  failed: number;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  const summary = useId();
  const total = missing + failed;

  useEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement as HTMLElement | null;
    element.showModal();
    const frame = requestAnimationFrame(() => element.classList.add("is-open"));
    return () => {
      cancelAnimationFrame(frame);
      element.close();
      if (previous?.isConnected) previous.focus();
    };
  }, []);

  return (
    <dialog
      ref={dialog}
      className="conversion-help-dialog t-modal"
      aria-labelledby={title}
      aria-describedby={summary}
      onCancel={event => { event.preventDefault(); onClose(); }}
      onClick={event => {
        if (event.target !== event.currentTarget) return;
        const rect = event.currentTarget.getBoundingClientRect();
        if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) onClose();
      }}
    >
      <header className="border-b border-edge px-5 py-4">
        <h2 id={title} className="text-[17px] font-medium text-ink">
          Remove {total} {total === 1 ? "entry" : "entries"}?
        </h2>
      </header>
      <div id={summary} className="px-5 py-4 text-[13px] leading-relaxed text-ink-dim">
        <ul className="mb-3 list-disc space-y-1 pl-5">
          {missing > 0 && <li>{missing} finished {missing === 1 ? "download whose file was" : "downloads whose files were"} moved or deleted</li>}
          {failed > 0 && <li>{failed} failed {failed === 1 ? "download" : "downloads"}</li>}
        </ul>
        <p>They are taken off the list. Files still on disk are not deleted. You can undo this for a few seconds afterwards.</p>
        <p className="mt-2 text-[12px] text-ink-faint">Downloads on a disconnected drive are left alone.</p>
      </div>
      <footer className="flex justify-end gap-2 border-t border-edge px-5 py-3">
        <Button tone="ghost" onClick={onClose}>Cancel</Button>
        <Button tone="danger" onClick={() => { onConfirm(); onClose(); }}>Remove {total}</Button>
      </footer>
    </dialog>
  );
}
