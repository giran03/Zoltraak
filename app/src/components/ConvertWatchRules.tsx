import { useCallback, useEffect, useState } from "react";
import { Button, ErrorStrip, Field, Switch, TextInput } from "./Controls";
import { Trash } from "./Icons";
import { Legend } from "./Panel";
import { engine, hasEngine, type ConversionSettings, type ConversionWatchRule, type ConversionWatchStatus } from "../lib/engine";

const REFRESH_MS = 10_000;
const DEFAULT_TYPES = "webm, mkv, mov";

/** Folder rules that convert new files with a saved recipe. Files already in the folder are never touched. */
export function ConvertWatchRules({ current, defaultFolder }: { current: ConversionSettings; defaultFolder: string | null }) {
  const [rules, setRules] = useState<ConversionWatchRule[]>([]);
  const [statuses, setStatuses] = useState<ConversionWatchStatus[]>([]);
  const [folder, setFolder] = useState("");
  const [types, setTypes] = useState(DEFAULT_TYPES);
  const [output, setOutput] = useState("");
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!hasEngine()) return;
    try { const loaded = await engine.listWatchRules(); setRules(loaded.rules); setStatuses(loaded.statuses); }
    catch (e) { setError(e instanceof Error ? e.message : String(e)); }
  }, []);
  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  async function save(next: ConversionWatchRule[]): Promise<boolean> {
    setError(null);
    try { await engine.saveWatchRules(next); setRules(next); return true; }
    catch (e) { setError(e instanceof Error ? e.message : String(e)); return false; }
  }
  async function add() {
    const extensions = types.split(/[\s,]+/).map((type) => type.replace(/^\./, "").toLowerCase()).filter(Boolean);
    const saved = await save([...rules, { id: crypto.randomUUID(), folder: folder.trim(), extensions, settings: current, outputDirectory: output.trim() || null, enabled: true }]);
    if (saved) setFolder("");
  }
  async function browse(setter: (path: string) => void) {
    try { const picked = await engine.chooseDirectory(defaultFolder ?? undefined); if (picked) setter(picked); }
    catch (e) { setError(e instanceof Error ? e.message : String(e)); }
  }

  return <div className="flex flex-col gap-4" data-testid="watch-rules">
    {error && <ErrorStrip message={error} onDismiss={() => setError(null)} />}
    {rules.length === 0 ? <p className="text-[13px] text-ink-dim">No folders are being watched.</p> :
      <div className="recess rounded-hw-lg">{rules.map((rule) => {
        const status = statuses.find((entry) => entry.ruleId === rule.id);
        return <div key={rule.id} className="flex items-start gap-3 border-b border-edge px-4 py-3 last:border-b-0">
          <Switch checked={rule.enabled} label={`Watch ${rule.folder}`} onChange={(enabled) => void save(rules.map((entry) => entry.id === rule.id ? { ...entry, enabled } : entry))} />
          <div className="min-w-0 flex-1">
            <p data-selectable className="truncate text-[13px] text-ink" title={rule.folder}>{rule.folder}</p>
            <p className="mt-1 text-[11px] text-ink-dim">.{rule.extensions.join(", .")} → {rule.settings.format.toUpperCase()}{rule.outputDirectory ? ` in ${rule.outputDirectory}` : " in a Converted subfolder"}</p>
            <p className={`mt-1 text-[11px] ${status?.lastError ? "text-danger" : "text-ink-faint"}`}>{status?.lastError ?? `${status?.queued ?? 0} converted since the app started`}</p>
          </div>
          <button type="button" className="focus-ring p-1 text-ink-dim" aria-label={`Stop watching ${rule.folder}`} onClick={() => void save(rules.filter((entry) => entry.id !== rule.id))}><Trash className="size-4" /></button>
        </div>;
      })}</div>}
    <div className="grid gap-4 sm:grid-cols-2">
      <Field label="Folder to watch" hint="New files that appear here are converted. What is already inside is left alone.">
        <div className="flex items-start gap-2"><TextInput value={folder} onChange={setFolder} ariaLabel="Folder to watch" placeholder={defaultFolder ?? "C:\\Users\\you\\Downloads"} mono className="flex-1" /><Button tone="ghost" onClick={() => void browse(setFolder)}>Browse</Button></div>
      </Field>
      <Field label="File types" hint="Extensions to match, separated by commas.">
        <TextInput value={types} onChange={setTypes} ariaLabel="File types to watch for" placeholder={DEFAULT_TYPES} mono />
      </Field>
      <Field label="Results folder" hint="Leave empty to use a Converted folder inside the watched one. It must differ from the watched folder.">
        <div className="flex items-start gap-2"><TextInput value={output} onChange={setOutput} ariaLabel="Results folder" placeholder="Watched folder/Converted" mono className="flex-1" /><Button tone="ghost" onClick={() => void browse(setOutput)}>Browse</Button></div>
      </Field>
    </div>
    <div className="flex flex-wrap items-center gap-3">
      <Button disabled={!folder.trim() || !types.trim()} onClick={() => void add()}>Watch with the current settings</Button>
      <Legend>Rules run while the app is open. A file is converted once it stops growing.</Legend>
    </div>
  </div>;
}
