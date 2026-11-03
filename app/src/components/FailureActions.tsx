import { useState } from "react";
import { recoveryFor, type RecoveryKind } from "../lib/downloadUx";
import type { QueueItem } from "../lib/snapshots";

export function FailureActions({ item, onRetry, onRecover }: {
  item: QueueItem; onRetry: () => Promise<unknown>;
  onRecover?: (item: QueueItem, kind: RecoveryKind) => Promise<void>;
}) {
  const recovery = recoveryFor(item);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState("");
  async function run(action: () => Promise<unknown>) {
    if (busy) return;
    setBusy(true); setFeedback("");
    try { if (await action() === false) setFeedback("The action failed. Check the download details and try again."); }
    catch (error) { setFeedback(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  }
  const button = "focus-ring rounded-hw border border-edge-strong px-2.5 py-1 text-[11px] text-ink hover:bg-face disabled:opacity-50";
  return <div className="border-t border-edge bg-danger/5 px-4 py-2.5" aria-label={`Recovery for ${item.filename}`}>
    <p className="text-[12px] text-ink">{recovery.reason}</p>
    <div className="mt-2 flex flex-wrap items-center gap-2">
      {recovery.kind !== "retry" && onRecover && <button type="button" className={button} disabled={busy} onClick={() => void run(() => onRecover(item, recovery.kind))}>{recovery.label}</button>}
      <button type="button" className={button} disabled={busy} onClick={() => void run(onRetry)}>Retry download</button>
      <details className="text-[11px] text-ink-dim"><summary className="focus-ring cursor-pointer">Technical details</summary>
        <p data-selectable className="mt-2 max-w-[75ch] break-all">{item.error || "No further details were provided."}</p>
        <button type="button" className={`${button} mt-2`} onClick={() => void run(async () => { await navigator.clipboard.writeText(`${item.filename}\n${item.url}\n${item.error ?? ""}`); setFeedback("Details copied."); })} disabled={busy}>Copy details</button>
      </details>
    </div>
    {feedback && <p role="status" className="mt-2 text-[11px] text-ink">{feedback}</p>}
  </div>;
}
