import { useEffect, useRef, useState } from "react";
import { engine } from "../lib/engine";
import { reviewLinks, type AddResult, type BatchResult } from "../lib/downloadUx";
import { Legend } from "./Panel";
import { AppVersion } from "./AppVersion";
import { useShortcuts } from "../lib/useShortcuts";
import { shortcutLabel } from "../lib/shortcuts";

/**
 * A path's tail is what identifies it; the drive letter is not. Trimming from
 * the front keeps the folder someone actually chose readable in a slot that is
 * never wide enough for the whole path.
 */
function shortenPath(path: string, max = 38): string {
  if (path.length <= max) return path;
  const sep = path.includes("\\") ? "\\" : "/";
  const parts = path.split(sep).filter(Boolean);
  let tail = "";
  for (let i = parts.length - 1; i >= 0; i -= 1) {
    const next = parts[i] + (tail ? sep + tail : "");
    if (next.length > max) break;
    tail = next;
  }
  return "…" + sep + (tail || parts[parts.length - 1]);
}

/**
 * THE ENTRY.
 *
 * The entry line on the control desk: docked flush to the bottom edge, always
 * present, never a modal and never a pill that has to be found and clicked
 * open first. Adding a download is the app's most common action, so the field
 * for it is furniture, not a destination.
 *
 * It takes a list, not a URL. Paste twenty links and twenty downloads start;
 * the field grows into a textarea the moment the content has more than one
 * line, so the single-link case still looks and behaves like a single field.
 *
 * `open` no longer decides whether the field exists — it only decides whether
 * the field has focus, which is what the `N` shortcut and the extension's
 * prefill actually want.
 */
export function AddDownload({
  open,
  onOpenChange,
  onSubmit,
  destination,
  onWorkingDirectoryChange,
  disabled,
  prefill,
  inputRef: externalRef,
  existingDownloads = [],
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSubmit: (urls: string[], directory?: string) => Promise<AddResult>;
  destination: string | null;
  onWorkingDirectoryChange?: (directory: string | null) => void;
  disabled?: boolean;
  prefill?: { url: string; headers: [string, string][]; nonce: number; directory?: string } | null;
  inputRef?: React.RefObject<HTMLTextAreaElement | null>;
  existingDownloads?: { url: string; directory: string }[];
}) {
  const shortcuts = useShortcuts();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [added, setAdded] = useState(0);
  // A folder chosen for this run of adds, overriding the configured default.
  // It outlives a submit on purpose: someone filing a batch into one folder
  // should not have to re-pick it for every link.
  const [chosen, setChosen] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  const localRef = useRef<HTMLTextAreaElement>(null);
  const inputRef = externalRef ?? localRef;

  const [reviewing, setReviewing] = useState(false);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [failures, setFailures] = useState<BatchResult["failed"]>([]);
  const target = chosen ?? destination;
  useEffect(() => { onWorkingDirectoryChange?.(target); }, [target, onWorkingDirectoryChange]);
  const existingUrls = existingDownloads.filter(item => !chosen || item.directory.replace(/\\/g, "/").toLowerCase() === chosen.replace(/\\/g, "/").toLowerCase()).map(item => item.url);
  const entries = reviewLinks(text, existingUrls);
  const urls = entries.filter(entry => entry.status === "ready").map(entry => entry.url!);
  const requiresReview = entries.length > 1 || entries.some(entry => entry.status !== "ready");
  const selectedUrls = reviewing ? entries.filter((entry, index) => entry.status === "ready" && selected.has(index)).map(entry => entry.url!) : urls;
  const multiline = text.includes("\n") || entries.length > 1;
  useEffect(() => {
    if (!added) return;
    const timer = setTimeout(() => setAdded(0), 5000);
    return () => clearTimeout(timer);
  }, [added]);

  useEffect(() => {
    if (open) inputRef.current?.focus();
  }, [open, inputRef]);

  useEffect(() => {
    if (!prefill?.url) return;
    setText(prefill.url);
    setChosen(prefill.directory ?? null);
    setReviewing(false); setFailures([]); setAdded(0);
    if (!open) onOpenChange(true);
    setTimeout(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    }, 50);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prefill?.nonce]);

  async function submit() {
    if (!entries.length || busy || disabled) return;
    if (requiresReview && !reviewing) {
      setSelected(new Set(entries.flatMap((entry, index) => entry.status === "ready" ? [index] : [])));
      setReviewing(true);
      return;
    }
    if (!selectedUrls.length) return;
    setBusy(true); setAdded(0); setFailures([]);
    try {
      const response = await onSubmit(selectedUrls, chosen ?? undefined);
      const result: BatchResult = typeof response === "boolean"
        ? { added: response ? selectedUrls : [], failed: response ? [] : selectedUrls.map(url => ({ url, error: "The engine rejected this link." })) }
        : response;
      const accepted = new Set(result.added);
      const remaining = entries.filter(entry => entry.status === "invalid" || entry.status === "ready" && !accepted.has(entry.url!));
      setText(remaining.map(entry => entry.raw).join("\n"));
      setFailures(result.failed); setAdded(result.added.length); setReviewing(false);
      if (!remaining.length) onOpenChange(false);
    } catch (error) {
      setFailures(selectedUrls.map(url => ({ url, error: error instanceof Error ? error.message : String(error) })));
    } finally { setBusy(false); }
  }

  const label = busy ? "Adding…" : requiresReview && !reviewing ? "Review links" : selectedUrls.length > 1 ? `Add ${selectedUrls.length} downloads` : "Add download";

  // The picked folder wins over the configured default until it is cleared.
  // A pasted batch's count and a detached engine both outrank the destination
  // for that one slot: they change whether the entry works at all.
  const shownDestination = !disabled ? target : null;

  /**
   * The destination is furniture, not a modal step: the picker only opens when
   * someone reaches for it, and cancelling leaves the current choice standing.
   */
  async function choose() {
    if (picking) return;
    setPicking(true);
    try {
      const picked = await engine.chooseDirectory(target ?? undefined);
      if (picked) { setChosen(picked); setReviewing(false); }
    } catch {
      // A picker that refuses to open is not worth a toast over the entry
      // line — the destination shown is still the one that will be used.
    } finally {
      setPicking(false);
    }
  }

  const hint =
    urls.length > 1
      ? `${urls.length} links detected`
      : disabled
        ? "engine detached — the entry is closed"
        : "shift + enter for a new line";

  return (
    <section
      aria-label="The entry"
      className="shrink-0 border-t border-edge-strong bg-face px-4 py-2.5"
    >
      {(reviewing || failures.length > 0) && <div aria-label="Batch download review" className="mb-3 border-b border-edge pb-3">
        <div className="flex flex-wrap items-baseline justify-between gap-2">
          <h3 className="text-[13px] font-medium text-ink">Review downloads</h3>
          <p className="text-[11px] text-ink-dim">{entries.filter(entry => entry.status === "ready").length} ready · {entries.filter(entry => entry.status === "duplicate" || entry.status === "existing").length} duplicates skipped · {entries.filter(entry => entry.status === "invalid").length} invalid</p>
        </div>
        <p className="my-2 break-all text-[11px] text-ink-dim">Download folder: {target || "System default"}. Category rules may select a different folder.</p>
        <div className="max-h-48 overflow-y-auto divide-y divide-edge">
          {entries.map((entry, index) => <label key={`${index}-${entry.raw}`} className="flex items-start gap-2 py-1.5 text-[12px]">
            <input type="checkbox" className="focus-ring mt-0.5 accent-accent" aria-label={`Include ${entry.raw}`} checked={reviewing && entry.status === "ready" && selected.has(index)} disabled={busy || !reviewing || entry.status !== "ready"} onChange={() => setSelected(prev => { const next = new Set(prev); if (next.has(index)) next.delete(index); else next.add(index); return next; })}/>
            <span className="min-w-0 flex-1 break-all text-ink">{entry.raw}</span>
            <span className={entry.status === "invalid" ? "shrink-0 text-danger" : "shrink-0 text-ink-dim"}>{entry.status === "ready" ? "Ready" : entry.status === "duplicate" ? "Repeated link" : entry.status === "existing" ? "Already in downloads" : "Invalid URL"}</span>
          </label>)}
        </div>
        {failures.map(failure => <p key={failure.url} role="status" className="mt-2 break-all text-[11px] text-danger">{failure.url}: {failure.error}</p>)}
        <p className="mt-2 text-[11px] text-ink-dim">Unsubmitted and invalid links stay in the entry for correction.</p>
      </div>}
      {added > 0 && <p role="status" className="mb-2 text-[12px] text-clear">Added {added} {added === 1 ? "download" : "downloads"}.</p>}
      <div className="flex items-end gap-2">
        <Legend className="shrink-0 pb-[7px]">entry</Legend>

        <textarea
          ref={inputRef}
          value={text}
          onChange={(e) => { setText(e.target.value); setReviewing(false); setFailures([]); setAdded(0); }}
          onFocus={() => onOpenChange(true)}
          onKeyDown={(e) => {
            // Enter submits; Shift+Enter is a newline, so a list can still be
            // typed by hand rather than only pasted.
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void submit();
            }
            if (e.key === "Escape") {
              e.currentTarget.blur();
              onOpenChange(false);
            }
          }}
          disabled={disabled || busy}
          autoComplete="off"
          spellCheck={false}
          rows={multiline ? 4 : 1}
          placeholder={`https://…   paste one link or many${shortcuts.enabled && shortcuts.actions.newDownload.enabled ? `   ( ${shortcutLabel(shortcuts, "newDownload")} )` : ""}`}
          aria-label="Download URLs"
          className="recess focus-ring min-w-0 flex-1 resize-none rounded-hw px-2.5 py-1.5 font-mono text-[12px] leading-relaxed text-ink outline-none placeholder:text-ink-faint disabled:opacity-55"
        />

        <button
          type="button"
          onClick={() => void submit()}
          disabled={disabled || busy || !entries.length || reviewing && !selectedUrls.length}
          className={`focus-ring legend shrink-0 rounded-hw border px-4 py-[7px] text-[11px] transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${
            added > 0
              ? "border-clear bg-clear text-on-signal"
              : "border-route bg-route text-on-signal hover:brightness-110"
          }`}
        >
          {label}
        </button>
      </div>

      <div className="mt-1.5 flex items-center gap-2">
        <AppVersion />
        {/*
          Where the file lands, stated on the line that sends it and changeable
          from there — it occupies the hint's slot rather than crowding in
          beside it, because "where is this going" outranks a shortcut
          reminder. A destination worth showing is worth clicking: burying it
          in Settings makes "this one goes somewhere else" a trip away from the
          field you are already typing in.
        */}
        {shownDestination ? (
          <>
            <button
              type="button"
              onClick={() => void choose()}
              disabled={picking}
              title={`${shownDestination} — click to change`}
              aria-label={`Destination ${shownDestination}. Click to change.`}
              className="focus-ring group flex min-w-0 shrink items-center gap-1.5 rounded-hw px-1 py-0.5 disabled:cursor-wait"
            >
              <Legend className="shrink-0 group-hover:text-ink">
                {picking ? "choosing…" : "working to"}
              </Legend>
              <span className="describer min-w-0 truncate text-[11px] text-ink-faint group-hover:text-ink">
                {shortenPath(shownDestination)}
              </span>
            </button>
            {chosen && (
              <button
                type="button"
                onClick={() => { setChosen(null); setReviewing(false); }}
                title={destination ? `Back to ${destination}` : "Back to the default folder"}
                className="focus-ring legend shrink-0 rounded-hw px-1 py-0.5 text-[11px] text-ink-faint hover:text-ink"
              >
                × default
              </button>
            )}
          </>
        ) : (
          <Legend className={disabled ? "text-danger" : undefined}>{hint}</Legend>
        )}

        <span className="h-px min-w-4 flex-1 bg-edge" />
        <Legend className="hidden shrink-0 whitespace-nowrap lg:inline">
          {shortcuts.enabled ? ([['newDownload', 'add'], ['search', 'search'], ['selectAll', 'select all'], ['togglePause', 'pause'], ['remove', 'remove']] as const)
            .filter(([id]) => shortcuts.actions[id].enabled).map(([id, label]) => `${shortcutLabel(shortcuts, id)} ${label}`).join(" · ") || "Shortcuts off" : "Shortcuts off"}
        </Legend>
      </div>
    </section>
  );
}
