import { useEffect, useRef } from "react";
import { SUPPORTED_FORMATS } from "../lib/fileDrop";

const SHOWN = 8;

/**
 * Shown when files dropped on the Convert tab are not something it can convert.
 * Supported files dropped alongside them are still added; `addedCount` says so,
 * so the message never leaves anyone wondering what happened to the rest.
 */
export function UnsupportedFilesModal({ names, addedCount, onClose }: {
  names: string[];
  addedCount: number;
  onClose: () => void;
}) {
  const close = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    close.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const hidden = Math.max(0, names.length - SHOWN);
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="unsupported-files-title"
      className="modal-backdrop fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="t-modal is-open plate modal-plate glass-panel flex max-h-[calc(100dvh-32px)] w-full max-w-md flex-col gap-4 overflow-y-auto rounded-hw-modal border border-edge-strong bg-face p-5 shadow-2xl">
        <div>
          <h2 id="unsupported-files-title" className="legend text-[12px] text-ink">
            {names.length === 1 ? "This file can't be converted" : `${names.length} files can't be converted`}
          </h2>
          <p className="mt-1.5 text-[12px] leading-relaxed text-ink-dim">
            {addedCount > 0
              ? `${addedCount === 1 ? "The supported item was" : `The ${addedCount} supported items were`} added. ${names.length === 1 ? "This one was" : "These were"} left out.`
              : "Nothing was added. Drop video, audio, images or animations."}
          </p>
        </div>

        <ul className="recess flex flex-col rounded-hw-lg text-[12px] text-ink">
          {names.slice(0, SHOWN).map((name, index) => (
            <li key={`${name}-${index}`} className="truncate border-b border-edge px-3 py-1.5 last:border-b-0" title={name}>{name}</li>
          ))}
          {hidden > 0 && <li className="px-3 py-1.5 text-ink-faint">and {hidden} more</li>}
        </ul>

        <div className="flex flex-col gap-1.5">
          <span className="legend text-[11px] text-ink-faint">Supported</span>
          {SUPPORTED_FORMATS.map((group) => (
            <p key={group.kind} className="text-[12px] leading-snug text-ink-dim">
              <span className="text-ink">{group.kind}</span> · {group.formats}
            </p>
          ))}
        </div>

        <div className="flex justify-end">
          <button
            ref={close}
            type="button"
            onClick={onClose}
            className="focus-ring legend rounded-hw border border-edge-strong bg-recess px-4 py-1.5 text-[11px] text-ink transition-colors hover:bg-panel"
          >
            OK
          </button>
        </div>
      </div>
    </div>
  );
}
