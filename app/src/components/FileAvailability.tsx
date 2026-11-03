import { useEffect, useState } from "react";
import { engine, hasEngine, type OutputStatus } from "../lib/engine";
import { notifyFileStatusChanged } from "../lib/useFileStatuses";
import { Button } from "./Controls";

/** Checks only completed, opened details; focus and periodic refresh cover external moves. */
export function FileAvailability({ id, onRemoveEntry }: { id: string; onRemoveEntry?: () => void }) {
  const [file, setFile] = useState<OutputStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    if (!hasEngine()) return;
    let active = true;
    let request = 0;
    const check = () => {
      if (document.hidden) return;
      const current = ++request;
      void engine.fileStatus(id).then(result => {
        if (active && current === request) {
          setFile(result);
          setError(null);
          notifyFileStatusChanged(id, result);
        }
      })
        .catch(e => { if (active && current === request) { setFile(null); setError(String(e)); } });
    };
    check();
    window.addEventListener("focus", check);
    document.addEventListener("visibilitychange", check);
    const timer = window.setInterval(check, 60_000);
    return () => { active = false; clearInterval(timer); window.removeEventListener("focus", check); document.removeEventListener("visibilitychange", check); };
  }, [id, revision]);

  async function act(action: "locate" | "again") {
    setBusy(true); setError(null);
    try {
      if (action === "locate") { await engine.locateFile(id); }
      else { await engine.redownloadMissing(id); }
      setRevision(value => value + 1);
      notifyFileStatusChanged(id);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  return <div className="mt-3 border-t border-edge pt-3 text-[12px]" aria-label="File availability">
    <p role="status" className={file?.status === "present" ? "text-ink-dim" : "text-danger"}>
      {error ? `Could not check or update file: ${error}` : !hasEngine() ? "File checks require the desktop app." : !file ? "Checking file…" : file.status === "present" ? "File available on disk" : file.detail || "File availability unknown"}
    </p>
    {file?.path && <p className="mt-1 break-all text-ink-dim" data-selectable>{file.path}</p>}
    <div className="mt-2 flex flex-wrap gap-2">
      {file && file.status !== "present" && <Button tone="ghost" disabled={busy} onClick={() => void act("locate")}>Locate file</Button>}
      {file?.status === "missing" && <Button disabled={busy} onClick={() => void act("again")}>Download again</Button>}
      {file && file.status !== "present" && onRemoveEntry && <Button tone="danger" disabled={busy} onClick={onRemoveEntry}>Remove entry only</Button>}
      <Button tone="ghost" disabled={busy || !hasEngine()} onClick={() => setRevision(value => value + 1)}>Check again</Button>
    </div>
    {file && file.status !== "present" && <p className="mt-2 text-[11px] text-ink-faint">Locate links the file you select to this entry. It does not verify its contents against the original download.</p>}
  </div>;
}
